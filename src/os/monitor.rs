use crate::core::{config::Limits, limits::read_bounded};
use anyhow::{Result, bail};
use reqwest::blocking::{Client, RequestBuilder};
#[derive(Debug)]
pub struct RetryAfter(pub u64);
impl std::fmt::Display for RetryAfter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "service rate limited; retry after {} seconds", self.0)
    }
}
impl std::error::Error for RetryAfter {}
pub struct Http {
    pub client: Client,
    pub max_bytes: u64,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
impl Http {
    pub fn new(limits: &Limits) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(limits.request_timeout_secs))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("lion-assist/0.1 (personal aerospace research)")
                .build()?,
            max_bytes: limits.max_response_bytes,
            cancel: None,
        })
    }
    pub fn set_cancel(&mut self, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.cancel = Some(cancel);
    }
    pub fn send(&self, request: RequestBuilder) -> Result<Vec<u8>> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            bail!("shutdown requested before HTTP call");
        }
        let response = request.send().map_err(|e| e.without_url())?;
        if response.status().as_u16() == 429 {
            let header = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.parse::<f64>().ok());
            let bytes = read_bounded(response, self.max_bytes)?;
            let body = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("retry_after").and_then(|n| n.as_f64()));
            let seconds = header
                .or(body)
                .filter(|n| n.is_finite() && *n >= 0.0)
                .unwrap_or(60.0)
                .ceil() as u64;
            return Err(RetryAfter(seconds.max(1)).into());
        }
        if !response.status().is_success() {
            bail!(
                "HTTP {}; request not retried (respect Retry-After before rerunning)",
                response.status()
            );
        }
        read_bounded(response, self.max_bytes)
    }
}
