use std::fmt::Write as _;
use std::time::Duration;

use ureq::Agent;
use ureq::http::Response;

use crate::error::ApiError;
use crate::native::{origin_allowed, resolve_url};

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_REDIRECTS: usize = 5;

pub(crate) fn build_agent(body_timeout: Duration, global_timeout: Option<Duration>) -> Agent {
    let mut cfg = Agent::config_builder()
        .max_redirects(0)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(READ_TIMEOUT))
        .timeout_per_read(Some(READ_TIMEOUT))
        .timeout_recv_body(Some(body_timeout));
    if let Some(global) = global_timeout {
        cfg = cfg.timeout_global(Some(global));
    }
    cfg.build().into()
}

pub(crate) fn map_transport_error(e: ureq::Error, url: &str) -> ApiError {
    match e {
        ureq::Error::StatusCode(status) => ApiError::Http {
            status,
            url: url.to_owned(),
        },
        other => ApiError::Network(other.to_string()),
    }
}

pub(crate) fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

pub(crate) fn get_with_redirects(
    agent: &Agent,
    url: &str,
    headers: &[(&str, &str)],
    allowed_origins: Option<&[String]>,
) -> Result<Response<ureq::Body>, ApiError> {
    let mut current = url.to_owned();
    for hops in 0..=MAX_REDIRECTS {
        if let Some(origins) = allowed_origins
            && !origin_allowed(&current, origins)
        {
            return Err(ApiError::Parse(format!(
                "redirect target is outside the source's allowed origins: {current}"
            )));
        }
        let mut req = agent.get(&current);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let resp = req.call().map_err(|e| map_transport_error(e, &current))?;
        let status = resp.status().as_u16();
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Ok(resp);
        }
        if hops == MAX_REDIRECTS {
            return Err(ApiError::Network(format!("too many redirects for {url}")));
        }
        let Some(location) = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        else {
            return Err(ApiError::Http {
                status,
                url: current,
            });
        };
        let next = resolve_url(&current, location);
        if is_https(&current) && !is_https(&next) {
            return Err(ApiError::Network(format!(
                "refused redirect downgrade from HTTPS to {next}"
            )));
        }
        current = next;
    }
    Err(ApiError::Network(format!("too many redirects for {url}")))
}

fn is_https(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

pub(crate) fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    use serde::Deserialize as _;
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
