use super::{
    config::Command,
    runtime::{Runtime, voice_command},
    state::now,
};
use crate::{
    core::{config::Config, limits::read_bounded},
    voice::engine,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};

pub struct Controls {
    pub paused: AtomicBool,
    pub away: AtomicBool,
    pub stop: Arc<AtomicBool>,
}
impl Controls {
    pub fn new(paused: bool, away: bool) -> Self {
        Self {
            paused: AtomicBool::new(paused),
            away: AtomicBool::new(away),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
    fn immediate(&self, command: &Command) {
        match command {
            Command::Pause => self.paused.store(true, Ordering::SeqCst),
            Command::Resume => self.paused.store(false, Ordering::SeqCst),
            Command::Away { enabled } => self.away.store(*enabled, Ordering::SeqCst),
            Command::Stop => self.stop.store(true, Ordering::SeqCst),
            _ => {}
        }
    }
}
enum Audio {
    Speak(String),
    Listen,
}
fn audio_loop(
    config: crate::core::config::Voice,
    settings: super::config::AgentVoice,
    rx: Receiver<Audio>,
    tx: SyncSender<Command>,
    controls: Arc<Controls>,
) {
    while !controls.stop.load(Ordering::SeqCst) {
        let event = rx.recv_timeout(Duration::from_millis(200));
        match event {
            Ok(Audio::Speak(text)) => {
                if let Err(error) = engine::speak(&config, &text) {
                    eprintln!("speech failed: {error:#}");
                }
                continue;
            }
            Ok(Audio::Listen) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if settings.continuous => {}
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // Listening and playback share this single worker, preventing self-triggering.
        match engine::listen(&config) {
            Ok(text) => {
                if let Some(command) = voice_command(&text, &settings.wake_phrase) {
                    // Apply controls only after admission, so a full queue cannot lose persistence.
                    if tx.try_send(command.clone()).is_ok() {
                        controls.immediate(&command);
                    }
                }
            }
            Err(error) => {
                eprintln!("listening failed: {error:#}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}
#[cfg(unix)]
fn socket_path(config: &Config) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = &config.agent.control_dir;
    if !dir.exists() {
        std::fs::DirBuilder::new().recursive(true).create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let meta = std::fs::symlink_metadata(dir)?;
    ensure!(
        meta.file_type().is_dir()
            && meta.uid() == nix::unistd::getuid().as_raw()
            && meta.mode() & 0o077 == 0,
        "control_dir must be a private directory owned by this user (mode 0700)"
    );
    let dir = dir.canonicalize()?;
    let data = if config.data_dir.exists() {
        config.data_dir.canonicalize()?
    } else {
        std::env::current_dir()?.join(&config.data_dir)
    };
    ensure!(
        !dir.starts_with(&data),
        "control_dir must be outside data_dir because the store forbids sockets"
    );
    let path = dir.join("agent.sock");
    ensure!(
        path.as_os_str().len() < 100,
        "control socket path is too long"
    );
    Ok(path)
}
#[cfg(unix)]
pub fn send(config: &Config, command: Command) -> Result<Value> {
    use std::{io::Read, net::Shutdown, os::unix::net::UnixStream};
    let mut stream = UnixStream::connect(socket_path(config)?)
        .context("agent is not running; start `daemon` first")?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&serde_json::to_vec(&command)?)?;
    stream.shutdown(Shutdown::Write)?;
    let mut bytes = Vec::new();
    stream
        .take(super::state::STATE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= super::state::STATE_LIMIT,
        "control response too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(not(unix))]
pub fn send(_config: &Config, _command: Command) -> Result<Value> {
    anyhow::bail!("resident control currently requires Unix")
}
#[cfg(unix)]
fn control_server(
    config: &Config,
    tx: SyncSender<Command>,
    snapshot: Arc<Mutex<Value>>,
    controls: Arc<Controls>,
) -> Result<std::thread::JoinHandle<()>> {
    use std::os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    };
    let path = socket_path(config)?;
    if path.exists() {
        ensure!(
            std::fs::symlink_metadata(&path)?.file_type().is_socket(),
            "control path is not a socket"
        );
        ensure!(
            UnixStream::connect(&path).is_err(),
            "another control server is active"
        );
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(std::thread::spawn(move || {
        while !controls.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let response=(||->Result<Value>{
                        let command:Command=serde_json::from_slice(&read_bounded(&mut stream,16384)?)?;
                        if matches!(command,Command::Status){
                            let mut value=snapshot.lock().map_err(|_|anyhow::anyhow!("status lock failed"))?.clone();
                            value["paused_requested"]=json!(controls.paused.load(Ordering::SeqCst));
                            value["away_requested"]=json!(controls.away.load(Ordering::SeqCst));
                            return Ok(value);
                        }
                        tx.try_send(command.clone()).context("control queue full; command not accepted")?;
                        controls.immediate(&command);
                        Ok(json!({"accepted":true,"durable":false,"note":"queued in the running process; status shows persisted jobs after processing"}))
                    })().unwrap_or_else(|e|json!({"error":format!("{e:#}")}));
                    let _ = serde_json::to_writer(&mut stream, &response);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(error) => {
                    eprintln!("control socket failed: {error}");
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(path);
    }))
}
#[cfg(unix)]
fn signals(controls: Arc<Controls>) -> Result<()> {
    use nix::sys::signal::{SigSet, Signal};
    let mut set = SigSet::empty();
    set.add(Signal::SIGINT);
    set.add(Signal::SIGTERM);
    set.thread_block()?;
    std::thread::spawn(move || {
        if set.wait().is_ok() {
            controls.stop.store(true, Ordering::SeqCst);
        }
    });
    Ok(())
}
pub fn run(config: Config, once: bool) -> Result<()> {
    // Block termination signals before reqwest creates its internal worker threads.
    #[cfg(unix)]
    let controls = Arc::new(Controls::new(false, config.agent.discord.away));
    #[cfg(unix)]
    if !once {
        signals(controls.clone())?;
    }
    let mut app = Runtime::new(config)?;
    #[cfg(unix)]
    {
        controls.paused.store(app.state.paused, Ordering::SeqCst);
        controls.away.store(app.state.away, Ordering::SeqCst);
        app.set_controls(controls);
    }
    if once {
        // One discovery/rundown and one attempt for each job present afterward.
        let limit = app.config.agent.max_jobs;
        app.tick(now())?;
        for _ in 1..limit {
            if !app
                .state
                .jobs
                .iter()
                .any(|j| j.status == super::state::Status::Pending && j.next_attempt <= now())
            {
                break;
            }
            app.tick(now())?;
        }
        println!("{}", app.status());
        return Ok(());
    }
    #[cfg(not(unix))]
    anyhow::bail!("resident daemon currently requires Unix; use --once for a batch");
    #[cfg(unix)]
    {
        let (commands_tx, commands_rx) = mpsc::sync_channel(32);
        let snapshot = Arc::new(Mutex::new(app.status()));
        let server = control_server(
            &app.config,
            commands_tx.clone(),
            snapshot.clone(),
            app.controls.clone(),
        )?;
        let (audio_tx, audio_rx) = mpsc::sync_channel(8);
        let voice = if app.config.voice.enabled {
            let config = app.config.voice.clone();
            let settings = app.config.agent.voice.clone();
            let controls = app.controls.clone();
            Some(std::thread::spawn(move || {
                audio_loop(config, settings, audio_rx, commands_tx, controls)
            }))
        } else {
            drop(audio_rx);
            None
        };
        println!(
            "{}",
            json!({"agent":"running","control_dir":app.config.agent.control_dir})
        );
        let result = (|| -> Result<()> {
            while !app.controls.stop.load(Ordering::SeqCst) {
                while let Ok(command) = commands_rx.try_recv() {
                    let requested = command.clone();
                    let response = if matches!(command, Command::Listen) {
                        audio_tx
                            .try_send(Audio::Listen)
                            .map(|_| json!({"listening":true}))
                            .map_err(anyhow::Error::from)
                    } else {
                        app.command(command, now())
                    };
                    match response {
                        Ok(value) => {
                            println!("{value}");
                            if app.config.voice.enabled
                                && let Some(spoken) = acknowledgement(&requested, &app)
                            {
                                let _ = audio_tx.try_send(Audio::Speak(spoken));
                            }
                        }
                        Err(error) => eprintln!("command failed: {error:#}"),
                    }
                }
                if app.stopped {
                    break;
                }
                app.tick(now())?;
                if app.config.voice.enabled
                    && let Some(item) = app.take_announcement(now())?
                    && audio_tx.try_send(Audio::Speak(item.text)).is_err()
                {
                    eprintln!("speech queue full; event remains in activity history");
                }
                *snapshot
                    .lock()
                    .map_err(|_| anyhow::anyhow!("status lock failed"))? = app.status();
                std::thread::sleep(Duration::from_millis(200));
            }
            app.state.paused = app.controls.paused.load(Ordering::SeqCst);
            app.state.away = app.controls.away.load(Ordering::SeqCst);
            app.save()
        })();
        app.controls.stop.store(true, Ordering::SeqCst);
        drop(audio_tx);
        let _ = server.join();
        if let Some(voice) = voice {
            let _ = voice.join();
        }
        result
    }
}

pub fn doctor(config: &Config) -> Value {
    fn installed(argv: &[String]) -> bool {
        argv.first().is_some_and(|name| {
            if name.contains('/') {
                std::path::Path::new(name).is_file()
            } else {
                std::env::var_os("PATH").is_some_and(|paths| {
                    std::env::split_paths(&paths).any(|path| path.join(name).is_file())
                })
            }
        })
    }
    fn key(name: &str) -> bool {
        std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
    }
    json!({
        "cheap_model_key_present":key(&config.models.cheap.api_key_env),
        "advanced_model_key_present":key(&config.models.advanced.api_key_env),
        "maildir_configured":config.agent.email.maildir,
        "maildir_exists":config.agent.email.maildir.as_ref().is_some_and(|p|p.join("new").is_dir() || p.join("cur").is_dir()),
        "email_moves_enabled":config.agent.email.apply_moves,
        "discord_enabled":config.agent.discord.enabled,
        "discord_token_present":key(&config.agent.discord.token_env),
        "discord_send_enabled":config.agent.discord.allow_send,
        "research_sources":config.agent.feeds.len()+config.agent.research_queries.len(),
        "voice_enabled":config.voice.enabled,
        "speech_executable_found":installed(&config.voice.speak_command),
        "listen_executable_found":installed(&config.voice.listen_command),
        "continuous_listening":config.agent.voice.continuous,
        "control_dir":config.agent.control_dir,
        "note":"Local checks only; credentials, remote permissions and audio hardware have not been tested."
    })
}

fn acknowledgement(command: &Command, app: &Runtime) -> Option<String> {
    Some(match command {
        Command::Status => format!(
            "I have {} tasks waiting and {} recent completed tasks. Away mode is {}.",
            app.state.jobs.len(),
            app.state.recent.len(),
            if app.state.away { "on" } else { "off" }
        ),
        Command::Pause => "I've paused scheduled work. Say Lion, resume when you're ready.".into(),
        Command::Resume => "I've resumed scheduled work.".into(),
        Command::Away { enabled } => {
            format!("Away mode is {}.", if *enabled { "on" } else { "off" })
        }
        Command::Rundown => "I've scheduled a rundown.".into(),
        Command::Ask { .. } => "I'll work on that.".into(),
        Command::Research { .. } => "I'll look for relevant research.".into(),
        Command::Undo { .. } | Command::UndoLast => "I've queued the email restore.".into(),
        Command::Retry { .. } | Command::Reprocess { .. } => "I've queued another attempt.".into(),
        Command::Reconcile { .. } => "I'll check whether that reply was delivered.".into(),
        Command::Dismiss { .. } => "I've dismissed that task.".into(),
        Command::SummarizeDiscord { .. } => "I'll summarize the Discord conversation.".into(),
        Command::ReadLast | Command::Why | Command::Listen | Command::Stop => return None,
    })
}
