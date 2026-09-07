use crate::{
    core::{
        config::Config,
        limits::Store,
        models::{Complexity, Router, complexity_for},
        scheduler::Pacer,
    },
    os::monitor::Http,
    tasks::{aerospace, discord, email, task::Task},
    voice::engine,
};
use anyhow::Result;
use serde_json::{Value, json};
pub struct Controller {
    pub config: Config,
    pub store: Store,
    http: Http,
    router: Router,
    pacer: Pacer,
}
impl Controller {
    pub fn new(config: Config) -> Result<Self> {
        let store = Store::open(
            &config.data_dir,
            config.limits.disk_bytes,
            config.limits.reserve_bytes,
        )?;
        let http = Http::new(&config.limits)?;
        let pacer = Pacer::new(config.research.interval_secs);
        Ok(Self {
            config,
            store,
            http,
            router: Router::new(),
            pacer,
        })
    }
    pub fn announce(&self, record: &email::OrganizedEmail) {
        if self.config.voice.enabled {
            // Do not read authentication codes, sensitive subjects, or spoofed sender text aloud.
            if let Err(error) = engine::speak(
                &self.config.voice,
                &format!(
                    "Sir, a new tier {} email has arrived.",
                    record.tier.number()
                ),
            ) {
                eprintln!("announcement failed: {error:#}");
            }
        }
    }
    pub fn summary(&mut self, text: &str) -> Result<String> {
        self.router.ask(&self.config, &self.http, Complexity::Simple, "Summarize this conversation: main topics, decisions, unresolved questions and action items. Attribute statements; do not invent consensus.", text)
    }
    pub fn handle(&mut self, task: Task) -> Result<Value> {
        match task {
            Task::Status => Ok(
                json!({"used_bytes":self.store.used()?,"managed_budget_bytes":self.store.budget(),"total_limit_bytes":self.config.limits.disk_bytes,"os_quota_required_for_total_guarantee":true}),
            ),
            Task::Ask { text, complex } => {
                let complexity = complexity_for(&text, complex);
                let model = self.router.choose(&self.config, complexity).model.clone();
                let answer = self.router.ask(
                    &self.config,
                    &self.http,
                    complexity,
                    "Answer the user's request.",
                    &text,
                )?;
                Ok(json!({"answer": answer, "model": model}))
            }
            Task::Email { path } => {
                let (record, fresh) =
                    email::organize(&path, &mut self.store, self.config.limits.max_input_bytes)?;
                if fresh {
                    self.announce(&record);
                }
                Ok(json!({"email":record,"new":fresh}))
            }
            Task::Research { query } => Ok(
                json!({"saved_papers":aerospace::crawler::crawl(&self.config, &self.http, &mut self.store, &mut self.pacer, &query)?}),
            ),
            Task::Discord { channel } => {
                let text = discord::conversation(
                    &self.http,
                    &channel,
                    self.config.limits.max_prompt_bytes.saturating_sub(512),
                )?;
                Ok(json!({"summary":self.summary(&text)?}))
            }
            Task::Speak { text } => {
                engine::speak(&self.config.voice, &text)?;
                Ok(json!({"spoken":true}))
            }
            Task::Listen => {
                let transcript = engine::listen(&self.config.voice)?;
                // Spoken input can ask questions; it never becomes a filesystem or shell command.
                let answer = self.router.ask(
                    &self.config,
                    &self.http,
                    complexity_for(&transcript, false),
                    "Answer briefly for spoken playback.",
                    &transcript,
                )?;
                engine::speak(&self.config.voice, &answer)?;
                Ok(json!({"transcript":transcript,"answer":answer}))
            }
            Task::Quit => Ok(json!({"stopped":true})),
        }
    }
}
