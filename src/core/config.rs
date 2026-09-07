use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub agent: crate::agent::config::AgentConfig,
    pub data_dir: PathBuf,
    pub limits: Limits,
    pub models: Models,
    pub voice: Voice,
    pub research: Research,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub disk_bytes: u64,
    pub reserve_bytes: u64,
    pub max_input_bytes: u64,
    pub max_response_bytes: u64,
    pub max_prompt_bytes: usize,
    pub max_output_tokens: u32,
    pub max_model_calls_per_run: usize,
    pub request_timeout_secs: u64,
    pub poll_secs: u64,
    pub max_emails_per_tick: usize,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub cheap: Model,
    pub advanced: Model,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub provider: Provider,
    pub model: String,
    pub base_url: String,
    pub api_key_env: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Gemini,
    ChatCompletions,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Voice {
    pub enabled: bool,
    pub speak_command: Vec<String>,
    pub listen_command: Vec<String>,
    pub timeout_secs: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Research {
    pub max_results: usize,
    pub interval_secs: u64,
    #[serde(default)]
    pub download_pdfs: bool,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = toml::from_str(&fs::read_to_string(path)?)?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        self.agent.validate()?;
        ensure!(
            !self.agent.voice.continuous
                || (self.voice.enabled && !self.voice.listen_command.is_empty()),
            "continuous voice requires voice.enabled and a listen_command"
        );
        ensure!(
            self.limits.disk_bytes > 0 && self.limits.disk_bytes <= 50_000_000_000,
            "disk_bytes must be within 1..=50,000,000,000 (decimal GB)"
        );
        ensure!(
            self.limits.reserve_bytes >= 1_048_576
                && self.limits.reserve_bytes < self.limits.disk_bytes,
            "reserve_bytes must leave at least 1 MiB for overhead and be below disk_bytes"
        );
        ensure!(
            (1..=64 * 1024 * 1024).contains(&self.limits.max_input_bytes),
            "input limit must be 1..=64 MiB"
        );
        ensure!(
            (1..=64 * 1024 * 1024).contains(&self.limits.max_response_bytes),
            "response limit must be 1..=64 MiB"
        );
        ensure!(
            (1..=1_000_000).contains(&self.limits.max_prompt_bytes),
            "invalid prompt limit"
        );
        ensure!(
            (1..=32_768).contains(&self.limits.max_output_tokens),
            "invalid output token limit"
        );
        ensure!(
            self.limits.max_model_calls_per_run > 0,
            "model call budget must be positive"
        );
        ensure!(
            (1..=300).contains(&self.limits.request_timeout_secs),
            "request timeout must be 1..=300 seconds"
        );
        ensure!(
            self.limits.poll_secs >= 5 && (1..=1000).contains(&self.limits.max_emails_per_tick),
            "invalid polling limits"
        );
        ensure!(
            (1..=100).contains(&self.research.max_results) && self.research.interval_secs >= 3,
            "research requires 1..=100 results and >=3 seconds between requests"
        );
        ensure!(
            (1..=300).contains(&self.voice.timeout_secs),
            "invalid voice timeout"
        );
        for model in [&self.models.cheap, &self.models.advanced] {
            let url = reqwest::Url::parse(&model.base_url)?;
            ensure!(
                url.scheme() == "https"
                    || (url.scheme() == "http"
                        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
                "model endpoint must use HTTPS or localhost HTTP"
            );
            ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "model base URL must not contain credentials, query or fragment"
            );
            ensure!(
                !model.model.is_empty()
                    && model
                        .model
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c)),
                "invalid model identifier"
            );
        }
        Ok(())
    }
}
