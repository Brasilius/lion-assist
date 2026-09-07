use crate::core::limits::Store;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const STATE_FILE: &str = "agent-state.json";
pub const STATE_LIMIT: u64 = 32 * 1024 * 1024;
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Work {
    Email {
        path: String,
        hash: String,
    },
    Research {
        query: String,
    },
    Feed {
        url: String,
    },
    Paper {
        object: String,
    },
    Discord {
        channel: String,
        message: Box<super::discord::Message>,
    },
    DiscordSummary {
        channel: String,
    },
    Ask {
        text: String,
    },
    Undo {
        job_id: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Review,
    Done,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub work: Work,
    pub status: Status,
    pub attempts: u32,
    pub calls: usize,
    pub next_attempt: u64,
    pub created: u64,
    pub updated: u64,
    pub proposal: Option<Value>,
    #[serde(default)]
    pub policy: Option<Value>,
    /// Written before any external side effect. Recovery reconciles this record.
    pub action: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<String>,
}
impl Job {
    pub fn new(id: String, work: Work, time: u64) -> Self {
        Self {
            id,
            work,
            status: Status::Pending,
            attempts: 0,
            calls: 0,
            next_attempt: time,
            created: time,
            updated: time,
            proposal: None,
            policy: None,
            action: None,
            result: None,
            error: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announcement {
    pub id: String,
    pub text: String,
    pub detail: String,
    pub urgent: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub paused: bool,
    pub away: bool,
    pub next_poll: u64,
    pub next_rundown: u64,
    pub next_research: u64,
    pub budget_day: u64,
    pub model_calls: usize,
    pub token_reservations: usize,
    #[serde(default)]
    pub discord_backfill: BTreeMap<String, (String, String)>,
    #[serde(default)]
    pub discord_retry_at: u64,
    pub discord_cursors: BTreeMap<String, String>,
    pub jobs: Vec<Job>,
    pub announcements: Vec<Announcement>,
    pub recent: Vec<Job>,
    pub last_spoken: Option<Announcement>,
    pub connector_errors: BTreeMap<String, String>,
}
impl State {
    pub fn load(store: &Store, away: bool) -> Result<Self> {
        if store.contains(STATE_FILE)? {
            let mut value: Self = serde_json::from_slice(&store.get(STATE_FILE, STATE_LIMIT)?)?;
            ensure!(value.version == 1, "unsupported agent state version");
            for job in &mut value.jobs {
                if job.status == Status::Running {
                    job.status = Status::Pending;
                }
            }
            Ok(value)
        } else {
            Ok(Self {
                version: 1,
                paused: false,
                away,
                next_poll: 0,
                next_rundown: 0,
                next_research: 0,
                budget_day: 0,
                model_calls: 0,
                token_reservations: 0,
                discord_cursors: BTreeMap::new(),
                discord_backfill: BTreeMap::new(),
                discord_retry_at: 0,
                jobs: vec![],
                announcements: vec![],
                recent: vec![],
                last_spoken: None,
                connector_errors: BTreeMap::new(),
            })
        }
    }
    pub fn save(&self, store: &mut Store) -> Result<()> {
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() as u64 <= STATE_LIMIT,
            "agent checkpoint exceeds size limit"
        );
        store.replace(STATE_FILE, &bytes)
    }
    pub fn reset_budget(&mut self, time: u64) {
        // A backwards wall-clock adjustment must not replenish the allowance.
        if time / 86400 > self.budget_day {
            self.budget_day = time / 86400;
            self.model_calls = 0;
            self.token_reservations = 0;
        }
    }
}
