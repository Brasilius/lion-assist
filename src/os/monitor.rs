use crate::core::{config::Limits, limits::read_bounded};
use anyhow::{Result, bail};
use reqwest::blocking::{Client, RequestBuilder};
pub struct Http {
    pub client: Client,
    pub max_bytes: u64,
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
        })
    }
    pub fn send(&self, request: RequestBuilder) -> Result<Vec<u8>> {
        let response = request.send().map_err(|e| e.without_url())?;
        if !response.status().is_success() {
            bail!(
                "HTTP {}; request not retried (respect Retry-After before rerunning)",
                response.status()
            );
        }
        read_bounded(response, self.max_bytes)
    }
}
