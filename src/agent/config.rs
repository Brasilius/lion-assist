use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub rundown_secs: u64,
    pub poll_secs: u64,
    pub research_secs: u64,
    pub max_jobs: usize,
    pub max_attempts: u32,
    pub daily_model_calls: usize,
    pub daily_token_budget: usize,
    pub reserved_email_calls: usize,
    pub max_calls_per_job: usize,
    pub control_dir: PathBuf,
    pub interests: String,
    pub research_queries: Vec<String>,
    pub feeds: Vec<String>,
    pub email: EmailConfig,
    pub discord: DiscordConfig,
    pub voice: AgentVoice,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            rundown_secs: 900,
            poll_secs: 30,
            research_secs: 21_600,
            max_jobs: 200,
            max_attempts: 3,
            daily_model_calls: 100,
            daily_token_budget: 1_000_000,
            reserved_email_calls: 10,
            max_calls_per_job: 4,
            control_dir: {
                #[cfg(unix)]
                let suffix = nix::unistd::getuid().as_raw().to_string();
                #[cfg(not(unix))]
                let suffix = "local".to_owned();
                std::env::temp_dir().join(format!("lion-assist-control-{suffix}"))
            },
            interests: "Aerospace engineering, spacecraft, aerodynamics and propulsion".into(),
            research_queries: vec![],
            feeds: vec![],
            email: EmailConfig::default(),
            discord: DiscordConfig::default(),
            voice: AgentVoice::default(),
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmailConfig {
    pub maildir: Option<PathBuf>,
    pub apply_moves: bool,
    pub policy: String,
    pub review_all: bool,
}
impl Default for EmailConfig {
    fn default() -> Self {
        Self { maildir: None, apply_moves: false, review_all: true,
            policy: "Keep financial, legal and security messages critical. Mark deadlines and personal requests important. Group aerospace newsletters as news. Preserve every message; never delete email.".into() }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscordConfig {
    pub enabled: bool,
    pub allow_send: bool,
    pub away: bool,
    pub channels: Vec<String>,
    pub bot_id: String,
    pub owner_name: String,
    pub reply_policy: String,
    pub api_base: String,
    pub token_env: String,
}
impl Default for DiscordConfig {
    fn default() -> Self {
        Self { enabled: false, allow_send: false, away: false, channels: vec![], bot_id: String::new(), owner_name: "the user".into(), reply_policy: "Answer factual questions addressed to the bot. Do not make commitments or disclose private information.".into(), api_base: "https://discord.com/api/v10".into(), token_env: "DISCORD_BOT_TOKEN".into() }
    }
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentVoice {
    pub continuous: bool,
    pub wake_phrase: String,
    /// UTC hours, avoiding dependency on a system timezone database.
    pub quiet_start_utc: Option<u8>,
    pub quiet_end_utc: Option<u8>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    Pause,
    Resume,
    Away { enabled: bool },
    Rundown,
    Ask { text: String },
    Research { query: String },
    Undo { job_id: String },
    Retry { job_id: String },
    Reconcile { job_id: String },
    Reprocess { job_id: String },
    Dismiss { job_id: String },
    UndoLast,
    SummarizeDiscord { channel: Option<String> },
    ReadLast,
    Why,
    Listen,
    Stop,
}
pub fn endpoint(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "endpoint requires HTTPS or localhost HTTP"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "endpoint cannot contain credentials, query or fragment"
    );
    Ok(())
}
impl AgentConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (5..=86400).contains(&self.poll_secs)
                && (5..=604800).contains(&self.rundown_secs)
                && (5..=604800).contains(&self.research_secs),
            "invalid agent intervals"
        );
        ensure!(
            (1..=1000).contains(&self.max_jobs) && (1..=10).contains(&self.max_attempts),
            "invalid queue limits"
        );
        ensure!(
            self.daily_model_calls > self.reserved_email_calls
                && self.daily_token_budget > 0
                && (2..=16).contains(&self.max_calls_per_job),
            "invalid agent model budget"
        );
        ensure!(
            self.research_queries.len() <= 20
                && self.feeds.len() <= 20
                && self.discord.channels.len() <= 20,
            "too many configured sources"
        );
        ensure!(
            self.interests.len() <= 4000
                && self.email.policy.len() <= 8000
                && self.discord.reply_policy.len() <= 4000,
            "policy too long"
        );
        for query in &self.research_queries {
            ensure!(
                !query.trim().is_empty() && query.len() <= 512,
                "invalid research query"
            );
        }
        for feed in &self.feeds {
            endpoint(feed)?;
        }
        endpoint(&self.discord.api_base)?;
        if self.discord.enabled {
            super::discord::snowflake(&self.discord.bot_id)?;
            ensure!(
                !self.discord.channels.is_empty(),
                "Discord requires configured channels"
            );
            for channel in &self.discord.channels {
                super::discord::snowflake(channel)?;
            }
        }
        ensure!(
            !self.discord.owner_name.is_empty()
                && self.discord.owner_name.len() <= 100
                && !self.discord.owner_name.contains(['\n', '\r', '@']),
            "invalid Discord owner name"
        );
        ensure!(
            matches!(
                (self.voice.quiet_start_utc, self.voice.quiet_end_utc),
                (None, None)
            ) || matches!((self.voice.quiet_start_utc, self.voice.quiet_end_utc), (Some(a),Some(b)) if a < 24 && b < 24 && a != b),
            "quiet hours require distinct start/end UTC hours within 0..23"
        );
        Ok(())
    }
}
