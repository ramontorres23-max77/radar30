//! Bounded synchronous HTTP client with retry-on-429, response size cap, and typed errors.

use std::fmt;
use std::io::Read;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::schema::SourceState;

/// Default User-Agent sent to public APIs.
pub const USER_AGENT: &str = concat!(
    "radar30/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/radar30/radar30)"
);

/// Hard upper bound on any single HTTP response body (4 MiB).
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Typed HTTP error mapped directly onto `SourceState`.
#[derive(Debug)]
pub enum HttpError {
    RateLimited { status: u16, detail: String },
    Forbidden { detail: String },
    Timeout { detail: String },
    Unreachable { detail: String },
    Status { status: u16, detail: String },
    Decode { detail: String },
}

impl HttpError {
    pub fn state(&self) -> SourceState {
        match self {
            HttpError::RateLimited { .. } => SourceState::RateLimited,
            HttpError::Forbidden { .. } => SourceState::Forbidden,
            HttpError::Timeout { .. } => SourceState::Timeout,
            HttpError::Unreachable { .. } => SourceState::Unreachable,
            HttpError::Status { .. } | HttpError::Decode { .. } => SourceState::Error,
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::RateLimited { status, detail } => {
                write!(f, "HTTP {status} rate limited: {detail}")
            }
            HttpError::Forbidden { detail } => write!(f, "HTTP 403 forbidden: {detail}"),
            HttpError::Timeout { detail } => write!(f, "timeout: {detail}"),
            HttpError::Unreachable { detail } => write!(f, "unreachable: {detail}"),
            HttpError::Status { status, detail } => write!(f, "HTTP {status}: {detail}"),
            HttpError::Decode { detail } => write!(f, "decode error: {detail}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// Perform a bounded HTTP GET returning UTF-8 text, retrying transient 429/422 responses
/// up to `max_retries` within `deadline`.
pub fn get_text(
    url: &str,
    headers: &[(&str, &str)],
    per_req_timeout: Duration,
    deadline: Instant,
    max_retries: usize,
) -> Result<String, HttpError> {
    let mut attempt = 0usize;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(HttpError::Timeout {
                detail: "deadline exceeded before request".to_string(),
            });
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout = per_req_timeout
            .min(remaining)
            .max(Duration::from_millis(250));

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();

        let mut req = agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }

        match req.call() {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                if status == 429 || status == 422 {
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .map(Duration::from_secs);
                    let body =
                        read_bounded(resp.body_mut().as_reader()).unwrap_or_else(|_| String::new());
                    let is_slowdown = status == 429
                        || body.to_ascii_lowercase().contains("slow down")
                        || body.to_ascii_lowercase().contains("timeout");
                    if is_slowdown && attempt < max_retries {
                        let backoff = retry_after
                            .unwrap_or_else(|| Duration::from_millis(800 * (1u64 << attempt)))
                            .min(Duration::from_secs(4));
                        if Instant::now() + backoff < deadline {
                            std::thread::sleep(backoff);
                            attempt += 1;
                            continue;
                        }
                    }
                    if is_slowdown {
                        return Err(HttpError::RateLimited {
                            status,
                            detail: truncate(&body, 160),
                        });
                    }
                    return Err(HttpError::Status {
                        status,
                        detail: truncate(&body, 160),
                    });
                }
                if status == 403 {
                    return Err(HttpError::Forbidden {
                        detail: "upstream refused request".to_string(),
                    });
                }
                if !(200..300).contains(&status) {
                    let body =
                        read_bounded(resp.body_mut().as_reader()).unwrap_or_else(|_| String::new());
                    return Err(HttpError::Status {
                        status,
                        detail: truncate(&body, 160),
                    });
                }
                return read_bounded(resp.body_mut().as_reader());
            }
            Err(err) => {
                let msg = err.to_string();
                let lower = msg.to_ascii_lowercase();
                if lower.contains("timeout") || lower.contains("timed out") {
                    return Err(HttpError::Timeout { detail: msg });
                }
                return Err(HttpError::Unreachable { detail: msg });
            }
        }
    }
}

/// Perform a bounded HTTP GET and deserialize JSON into `T`.
pub fn get_json<T: DeserializeOwned>(
    url: &str,
    headers: &[(&str, &str)],
    per_req_timeout: Duration,
    deadline: Instant,
    max_retries: usize,
) -> Result<T, HttpError> {
    let text = get_text(url, headers, per_req_timeout, deadline, max_retries)?;
    serde_json::from_str(&text).map_err(|e| HttpError::Decode {
        detail: format!("invalid JSON: {e}"),
    })
}

fn read_bounded<R: Read>(reader: R) -> Result<String, HttpError> {
    let mut buf = Vec::new();
    reader
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut buf)
        .map_err(|e| HttpError::Unreachable {
            detail: format!("read error: {e}"),
        })?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn truncate(s: &str, max_chars: usize) -> String {
    let compact: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        compact
    } else {
        let mut out: String = compact.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_deadline_fails_fast_without_network() {
        let past = Instant::now() - Duration::from_secs(1);
        let err = get_text(
            "https://example.invalid/",
            &[],
            Duration::from_secs(5),
            past,
            0,
        )
        .expect_err("expired deadline must fail");
        assert_eq!(err.state(), SourceState::Timeout);
    }
}
