//! Fetching and caching Dynamics 365 `$metadata` (Finance & Operations or Dataverse):
//! interactive Azure AD sign-in, download with progress, a local cache per
//! environment, and the app's saved settings.

pub mod auth;
pub mod cache;
pub mod download;
pub mod settings;

pub use auth::{AuthConfig, Token, sign_in_interactive};
pub use cache::{Cache, CacheEntry, StoreOutcome};
pub use download::{Progress, download_metadata};
pub use settings::{Environment, Settings};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("server returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("sign-in failed: {0}")]
    SignIn(String),
    #[error("sign-in timed out")]
    SignInTimeout,
    #[error("cancelled")]
    Cancelled,
    #[error("invalid environment URL `{0}`")]
    InvalidUrl(String),
    #[error("invalid settings or cache file: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Http(e.to_string())
    }
}

/// Normalises an environment URL to `https://host` (no trailing slash or path).
pub fn normalize_environment_url(url: &str) -> Result<String, Error> {
    let trimmed = url.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme).map_err(|_| Error::InvalidUrl(url.into()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::InvalidUrl(url.into()))?;
    Ok(match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

fn check_status(
    mut resp: ureq::http::Response<ureq::Body>,
) -> Result<ureq::http::Response<ureq::Body>, Error> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(resp);
    }
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    Err(Error::Status {
        status,
        body: body.chars().take(500).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_urls() {
        assert_eq!(
            normalize_environment_url("contoso.operations.dynamics.com").unwrap(),
            "https://contoso.operations.dynamics.com"
        );
        assert_eq!(
            normalize_environment_url(" https://contoso.operations.dynamics.com/data/ ").unwrap(),
            "https://contoso.operations.dynamics.com"
        );
        assert_eq!(
            normalize_environment_url("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(normalize_environment_url("https://").is_err());
    }
}
