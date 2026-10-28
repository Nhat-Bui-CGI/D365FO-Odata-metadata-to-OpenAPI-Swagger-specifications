//! Interactive Azure AD sign-in: OAuth 2.0 authorization code flow with PKCE
//! and a loopback redirect (`http://localhost:<port>`), as recommended for
//! desktop apps.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Error, agent, check_status};

pub const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Usually `https://login.microsoftonline.com`.
    pub authority: String,
    /// Tenant ID or domain, or `organizations`.
    pub tenant: String,
    /// Application (client) ID of a public-client app registration with
    /// `http://localhost` as a redirect URI.
    pub client_id: String,
    /// Normalised environment URL, used as the token audience.
    pub environment_url: String,
}

impl AuthConfig {
    fn endpoint(&self, name: &str) -> String {
        format!(
            "{}/{}/oauth2/v2.0/{name}",
            self.authority.trim_end_matches('/'),
            self.tenant
        )
    }

    fn scope(&self) -> String {
        format!(
            "{}/.default offline_access",
            self.environment_url.trim_end_matches('/')
        )
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: SystemTime,
}

impl Token {
    /// True if the token is expired or will be within a minute.
    pub fn is_expired(&self) -> bool {
        SystemTime::now() + Duration::from_secs(60) >= self.expires_at
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

/// Opens the system browser for sign-in and waits for the redirect.
///
/// `open_browser` is called with the sign-in URL (pass `|u| webbrowser::open(u)`);
/// it is a parameter so callers can also show the URL to copy.
/// Returns [`Error::Cancelled`] if `cancel` is set while waiting.
pub fn sign_in_interactive(
    cfg: &AuthConfig,
    timeout: Duration,
    cancel: &AtomicBool,
    open_browser: impl FnOnce(&str) -> std::io::Result<()>,
) -> Result<Token, Error> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect_uri = format!("http://localhost:{}", listener.local_addr()?.port());

    let verifier = random_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token();

    let mut url =
        url::Url::parse(&cfg.endpoint("authorize")).map_err(|e| Error::SignIn(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", &cfg.scope())
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    open_browser(url.as_str())?;

    let code = wait_for_code(&listener, &state, timeout, cancel)?;

    let resp = agent().post(&cfg.endpoint("token")).send_form([
        ("grant_type", "authorization_code"),
        ("client_id", cfg.client_id.as_str()),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
        ("scope", cfg.scope().as_str()),
    ])?;
    token_from(resp)
}

/// Gets a new access token with a refresh token, without opening the browser.
pub fn refresh(cfg: &AuthConfig, refresh_token: &str) -> Result<Token, Error> {
    let resp = agent().post(&cfg.endpoint("token")).send_form([
        ("grant_type", "refresh_token"),
        ("client_id", cfg.client_id.as_str()),
        ("refresh_token", refresh_token),
        ("scope", cfg.scope().as_str()),
    ])?;
    token_from(resp)
}

fn token_from(resp: ureq::http::Response<ureq::Body>) -> Result<Token, Error> {
    let body = check_status(resp)
        .map_err(|e| Error::SignIn(e.to_string()))?
        .body_mut()
        .read_to_string()?;
    let t: TokenResponse = serde_json::from_str(&body)?;
    Ok(Token {
        access_token: t.access_token,
        refresh_token: t.refresh_token,
        expires_at: SystemTime::now() + Duration::from_secs(t.expires_in),
    })
}

fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::SignInTimeout);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                // Browsers also probe for /favicon.ico etc.; keep waiting until the real redirect.
                if let Some(result) = handle_redirect(stream, state)? {
                    return result;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Reads one HTTP request. Returns `None` if it was not the OAuth redirect.
fn handle_redirect(
    mut stream: TcpStream,
    state: &str,
) -> Result<Option<Result<String, Error>>, Error> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let target = line.split_whitespace().nth(1).unwrap_or("/");
    let result = parse_redirect(target, state);
    let (title, message) = match &result {
        None => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return Ok(None);
        }
        Some(Ok(_)) => (
            "Signed in",
            "You are signed in. You can close this window and return to the app.",
        ),
        Some(Err(_)) => (
            "Sign-in failed",
            "Sign-in failed. Return to the app for details.",
        ),
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=utf-8><title>{title}</title></head><body style=\"font-family:sans-serif;margin:3em\"><h2>{title}</h2><p>{message}</p></body></html>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    Ok(result)
}

/// Extracts the authorization code from a redirect target like `/?code=..&state=..`.
fn parse_redirect(target: &str, expected_state: &str) -> Option<Result<String, Error>> {
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != "/" {
        return None;
    }
    let get = |k: &str| {
        url.query_pairs()
            .find(|(name, _)| name == k)
            .map(|(_, v)| v.into_owned())
    };
    if let Some(err) = get("error") {
        let desc = get("error_description").unwrap_or_default();
        return Some(Err(Error::SignIn(format!("{err}: {desc}"))));
    }
    let code = get("code")?;
    if get("state").as_deref() != Some(expected_state) {
        return Some(Err(Error::SignIn("state mismatch in redirect".into())));
    }
    Some(Ok(code))
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator unavailable");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_redirects() {
        assert_eq!(
            parse_redirect("/?code=abc&state=s1", "s1")
                .unwrap()
                .unwrap(),
            "abc"
        );
        assert!(
            parse_redirect("/?code=abc&state=other", "s1")
                .unwrap()
                .is_err()
        );
        assert!(
            parse_redirect("/?error=access_denied&error_description=nope", "s1")
                .unwrap()
                .is_err()
        );
        assert!(parse_redirect("/favicon.ico", "s1").is_none());
        assert!(parse_redirect("/", "s1").is_none());
    }

    #[test]
    fn pkce_values_are_url_safe() {
        let v = random_token();
        assert_eq!(v.len(), 43);
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(v, random_token());
    }

    #[test]
    fn loopback_redirect_is_captured() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            for path in ["/favicon.ico", "/?code=the-code&state=xyz"] {
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
                let mut out = String::new();
                let _ = std::io::Read::read_to_string(&mut s, &mut out);
            }
        });
        let code = wait_for_code(
            &listener,
            "xyz",
            Duration::from_secs(5),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(code, "the-code");
        client.join().unwrap();
    }

    fn config(authority: &str) -> AuthConfig {
        AuthConfig {
            authority: authority.into(),
            tenant: "organizations".into(),
            client_id: "client".into(),
            environment_url: "https://contoso.operations.dynamics.com".into(),
        }
    }

    #[test]
    fn refresh_parses_token_response() {
        let body = br#"{"token_type":"Bearer","access_token":"at","refresh_token":"rt","expires_in":3600}"#;
        let (url, server) = crate::download::tests::serve("200 OK", body.to_vec(), 1);
        let token = refresh(&config(&url), "old").unwrap();
        assert_eq!(
            (token.access_token.as_str(), token.refresh_token.as_deref()),
            ("at", Some("rt"))
        );
        assert!(!token.is_expired());
        server.join().unwrap();
    }

    #[test]
    fn token_errors_are_sign_in_errors() {
        let (url, server) = crate::download::tests::serve(
            "400 Bad Request",
            br#"{"error":"invalid_grant"}"#.to_vec(),
            1,
        );
        let err = refresh(&config(&url), "old").unwrap_err();
        assert!(
            matches!(&err, Error::SignIn(m) if m.contains("invalid_grant")),
            "{err}"
        );
        server.join().unwrap();
    }

    #[test]
    fn wait_can_be_cancelled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let err = wait_for_code(
            &listener,
            "x",
            Duration::from_secs(5),
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }
}
