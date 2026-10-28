//! Downloads `$metadata` with progress reporting.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, agent, check_status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Request sent, waiting for the server (D365FO can take a while to build metadata).
    Connecting,
    /// `total` is `None` when the server doesn't send a length.
    Downloading {
        received: u64,
        total: Option<u64>,
    },
    Done {
        bytes: u64,
    },
}

/// Downloads `{environment_url}/data/$metadata`.
pub fn download_metadata(
    environment_url: &str,
    access_token: &str,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<Vec<u8>, Error> {
    let url = format!("{}/data/$metadata", environment_url.trim_end_matches('/'));
    on_progress(Progress::Connecting);
    let resp = agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {access_token}"))
        .header("Accept", "application/xml")
        .call()?;
    let mut resp = check_status(resp)?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());

    let mut reader = resp.body_mut().as_reader();
    let mut data = Vec::with_capacity(total.unwrap_or(0).min(512 << 20) as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    on_progress(Progress::Downloading { received: 0, total });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        on_progress(Progress::Downloading {
            received: data.len() as u64,
            total,
        });
    }
    on_progress(Progress::Done {
        bytes: data.len() as u64,
    });
    Ok(data)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Serves one response per connection; returns the base URL and the received Authorization headers.
    pub fn serve(
        status: &'static str,
        body: Vec<u8>,
        connections: usize,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let handle = std::thread::spawn(move || {
            let mut auth = Vec::new();
            for _ in 0..connections {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                let mut request_len = 0;
                while r.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                    let lower = line.to_lowercase();
                    if lower.starts_with("authorization:") {
                        auth.push(line.trim().to_string());
                    }
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        request_len = v.trim().parse().unwrap();
                    }
                    line.clear();
                }
                // Drain the request body: on Windows, closing a socket with
                // unread data resets the connection before the client reads.
                std::io::Read::read_exact(&mut r, &mut vec![0; request_len]).unwrap();
                write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                s.write_all(&body).unwrap();
            }
            auth
        });
        (url, handle)
    }

    #[test]
    fn downloads_with_progress() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let (url, server) = serve("200 OK", body.clone(), 1);
        let mut events = Vec::new();
        let data =
            download_metadata(&url, "tok", &AtomicBool::new(false), |p| events.push(p)).unwrap();
        assert_eq!(data, body);
        assert_eq!(events.first(), Some(&Progress::Connecting));
        assert_eq!(events.last(), Some(&Progress::Done { bytes: 300_000 }));
        assert!(events.contains(&Progress::Downloading {
            received: 300_000,
            total: Some(300_000)
        }));
        let auth = server.join().unwrap();
        assert_eq!(auth.len(), 1);
        assert!(
            auth[0].eq_ignore_ascii_case("authorization: Bearer tok"),
            "{auth:?}"
        );
    }

    #[test]
    fn reports_http_errors() {
        let (url, server) = serve("401 Unauthorized", b"nope".to_vec(), 1);
        let err = download_metadata(&url, "tok", &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(matches!(err, Error::Status { status: 401, .. }), "{err}");
        server.join().unwrap();
    }
}
