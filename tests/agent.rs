use lion_assist::{
    agent::{
        config::Command,
        daemon, discord, email, research,
        runtime::{Runtime, voice_command},
        state::{Job, State, Status, Work},
    },
    core::{
        config::Config,
        limits::{Store, digest},
    },
    tasks::{
        aerospace::storage::{self, Paper},
        email::Tier,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Mock {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Mock {
    fn new(responses: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let thread = std::thread::spawn(move || {
            for (status, body) in responses {
                let start = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && start.elapsed() < Duration::from_secs(10) =>
                        {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => panic!("mock did not receive expected request: {e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(pos) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..pos]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|n| n.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= pos + 4 + len {
                            break;
                        }
                    }
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap());
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            base,
            requests,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> Vec<String> {
        self.thread.take().unwrap().join().unwrap();
        Arc::try_unwrap(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}
fn model(value: Value) -> (u16, Value) {
    (
        200,
        json!({"candidates":[{"content":{"parts":[{"text":value.to_string()}]}}]}),
    )
}
fn config(root: &Path) -> Config {
    let mut value = Config::load(Path::new("config/system.toml")).unwrap();
    value.data_dir = root.join("data");
    value.agent.control_dir = root.join("control");
    value.agent.research_queries.clear();
    value.agent.feeds.clear();
    value.agent.email.maildir = None;
    value.agent.discord.enabled = false;
    value.voice.enabled = false;
    value
}
fn runtime(mut config: Config, mock: Option<&Mock>) -> Runtime {
    if let Some(mock) = mock {
        config.models.cheap.base_url = mock.base.clone();
        config.models.advanced.base_url = mock.base.clone();
        config.models.cheap.api_key_env = "PATH".into();
        config.models.advanced.api_key_env = "PATH".into();
        config.agent.discord.api_base = mock.base.clone();
        config.agent.discord.token_env = "PATH".into();
    }
    let mut app = Runtime::new(config).unwrap();
    app.state.next_poll = u64::MAX;
    app.state.next_rundown = u64::MAX;
    app
}
fn mention() -> discord::Message {
    serde_json::from_value(json!({"id":"1000","channel_id":"123","author":{"id":"234","username":"friend"},"content":"<@999> What is an ion thruster?","mentions":[{"id":"999"}]})).unwrap()
}
fn discord_config(root: &Path) -> Config {
    let mut config = config(root);
    config.agent.discord.enabled = true;
    config.agent.discord.channels = vec!["123".into()];
    config.agent.discord.bot_id = "999".into();
    config.agent.discord.owner_name = "the user".into();
    config.agent.discord.away = true;
    config.agent.discord.allow_send = true;
    config
}
fn draft() -> Value {
    json!({"answer":"An ion thruster accelerates ions to produce thrust.","should_reply":true,"needs_review":false,"reason":"Factual question."})
}

#[test]
fn atomic_checkpoint_replaces_and_restarts_without_losing_state() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path(), 1_000_000, 10_000).unwrap();
    let mut state = State::load(&store, false).unwrap();
    let mut job = Job::new(
        "test".into(),
        Work::Ask {
            text: "hello".into(),
        },
        100,
    );
    job.status = Status::Running;
    job.calls = 2;
    job.action = Some(json!({"attempted":true}));
    state.jobs.push(job);
    state.model_calls = 9;
    state.save(&mut store).unwrap();
    state.away = true;
    state.save(&mut store).unwrap();
    drop(store);
    let store = Store::open(root.path(), 1_000_000, 10_000).unwrap();
    let restored = State::load(&store, false).unwrap();
    assert!(restored.away);
    assert_eq!(restored.jobs[0].status, Status::Pending);
    assert_eq!(restored.model_calls, 9);
    assert_eq!(restored.jobs[0].calls, 2);
    assert!(restored.jobs[0].action.is_some());
}
#[test]
fn checkpoint_quota_failure_preserves_previous_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path(), 100_000, 10_000).unwrap();
    store.replace("state.json", b"old").unwrap();
    assert!(store.replace("state.json", &vec![b'x'; 90_000]).is_err());
    assert_eq!(store.get("state.json", 100).unwrap(), b"old");
}
#[test]
fn full_model_email_decision_moves_and_undo_restores_exact_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mail = root.path().join("mail");
    fs::create_dir_all(mail.join("new")).unwrap();
    let source = mail.join("new/message");
    let content = b"From: colleague@example.test\nSubject: Meeting\n\nPlease respond by tomorrow.";
    fs::write(&source, content).unwrap();
    let decision = json!({"tier":"important","reason":"A colleague needs a response tomorrow.","evidence":["Please respond by tomorrow."],"needs_review":false});
    let mock = Mock::new(vec![model(decision.clone()), model(decision)]);
    let mut cfg = config(root.path());
    cfg.agent.email.maildir = Some(mail.clone());
    cfg.agent.email.apply_moves = true;
    let mut app = runtime(cfg, Some(&mock));
    app.enqueue(
        "email-test".into(),
        Work::Email {
            path: source.to_str().unwrap().into(),
            hash: digest(content),
        },
        100,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert!(app.state.jobs.is_empty());
    assert!(!source.exists());
    let job = &app.state.recent[0];
    let action: email::Move = serde_json::from_value(job.action.clone().unwrap()).unwrap();
    assert_eq!(fs::read(&action.destination).unwrap(), content);
    assert_eq!(app.state.model_calls, 2);
    assert_eq!(app.state.announcements.len(), 1);
    app.command(
        Command::Undo {
            job_id: "email-test".into(),
        },
        101,
    )
    .unwrap();
    app.tick(101).unwrap();
    assert_eq!(fs::read(source).unwrap(), content);
    assert!(!action.destination.exists());
    let cfg = config(root.path());
    drop(app);
    let mut restarted = runtime(cfg, None);
    assert_eq!(restarted.state.model_calls, 2);
    assert!(
        !restarted
            .enqueue(
                "email-test".into(),
                Work::Ask {
                    text: "duplicate".into()
                },
                102
            )
            .unwrap()
    );
    let requests = mock.finish();
    assert!(requests[0].contains("gemini-3-flash"));
    assert!(requests[1].contains("gemini-3.1-pro"));
}
#[test]
fn interrupted_mail_move_is_idempotent_and_conflicts_never_overwrite() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("new")).unwrap();
    let source = root.path().join("new/message");
    fs::write(&source, b"hello").unwrap();
    let movement = email::plan(root.path(), &source, &digest(b"hello"), Tier::News).unwrap();
    fs::hard_link(&source, &movement.destination).unwrap();
    email::apply(root.path(), &movement, 100).unwrap();
    email::apply(root.path(), &movement, 100).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read(&movement.destination).unwrap(), b"hello");
    fs::write(&source, b"different").unwrap();
    assert!(email::apply(root.path(), &movement, 100).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"different");
}
#[test]
fn invalid_model_evidence_cannot_move_email() {
    let root = tempfile::tempdir().unwrap();
    let mail = root.path().join("mail");
    fs::create_dir_all(mail.join("new")).unwrap();
    let source = mail.join("new/m");
    fs::write(&source, b"Subject: hello\n\nLunch?").unwrap();
    let decision = json!({"tier":"other","reason":"No deadline.","evidence":["fabricated evidence"],"needs_review":false});
    let mock = Mock::new(vec![model(decision.clone()), model(decision)]);
    let mut cfg = config(root.path());
    cfg.agent.email.maildir = Some(mail);
    cfg.agent.email.apply_moves = true;
    cfg.agent.max_attempts = 1;
    let mut app = runtime(cfg, Some(&mock));
    app.enqueue(
        "invalid".into(),
        Work::Email {
            path: source.to_str().unwrap().into(),
            hash: digest(&fs::read(&source).unwrap()),
        },
        100,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert_eq!(app.state.jobs[0].status, Status::Review);
    assert!(source.exists());
    mock.finish();
}
#[test]
fn daily_budget_is_durable_and_does_not_replenish_on_clock_rollback() {
    let root = tempfile::tempdir().unwrap();
    let cfg = config(root.path());
    let mut app = runtime(cfg, None);
    app.state.budget_day = 2;
    app.state.model_calls = app.config.agent.daily_model_calls;
    app.enqueue(
        "budget".into(),
        Work::Ask {
            text: "hello".into(),
        },
        172800,
    )
    .unwrap();
    app.tick(172800).unwrap();
    assert_eq!(app.state.jobs[0].status, Status::Pending);
    assert_eq!(app.state.jobs[0].calls, 0);
    assert_eq!(app.state.jobs[0].next_attempt, 259200);
    drop(app);
    let mut app = runtime(config(root.path()), None);
    app.state.reset_budget(86400);
    assert_eq!(app.state.model_calls, 100);
    app.state.reset_budget(259200);
    assert_eq!(app.state.model_calls, 0);
}
#[test]
fn cheap_research_selection_is_saved_once_and_announced_after_selection() {
    let root = tempfile::tempdir().unwrap();
    let mock = Mock::new(vec![model(
        json!({"relevant":true,"needs_review":false,"summary":"A new propulsion experiment.","reason":"Matches electric propulsion."}),
    )]);
    let mut app = runtime(config(root.path()), Some(&mock));
    let paper = Paper {
        source: "web".into(),
        id: "https://example.test/paper".into(),
        title: "Electric propulsion".into(),
        abstract_text: "A new experiment".into(),
        url: "https://example.test/paper".into(),
        published: "2026-09-01".into(),
    };
    storage::save(&mut app.store, &paper).unwrap();
    let object = app.store.list("paper-", 10).unwrap().remove(0);
    app.enqueue("selection".into(), Work::Paper { object }, 100)
        .unwrap();
    app.tick(100).unwrap();
    assert_eq!(app.store.list("reading-", 10).unwrap().len(), 1);
    assert_eq!(app.state.announcements.len(), 1);
    app.tick(101).unwrap();
    assert_eq!(app.state.announcements.len(), 1);
    assert_eq!(mock.finish().len(), 1);
}
#[test]
fn feeds_and_article_extraction_preserve_sources_and_exclude_scripts() {
    let rss=br#"<rss><channel><item><title>New engine</title><link>/engine</link><description>&lt;p&gt;Electric propulsion&lt;/p&gt;</description><pubDate>Today</pubDate></item></channel></rss>"#;
    let papers = research::parse("https://example.test/feed", rss, 10).unwrap();
    assert_eq!(papers.len(), 1);
    assert_eq!(papers[0].url, "https://example.test/engine");
    assert_eq!(papers[0].abstract_text, "Electric propulsion");
    let page=br#"<html><title>Aerospace article</title><body><nav>ignore nav</nav><article>Real <b>content</b><script>ignore script</script></article></body></html>"#;
    let papers = research::parse("https://example.test/article", page, 10).unwrap();
    assert_eq!(papers[0].abstract_text, "Real content");
}
#[test]
fn discord_reply_has_disclosure_no_pings_and_verified_delivery() {
    let root = tempfile::tempdir().unwrap();
    let cfg = discord_config(root.path());
    let message = mention();
    let body = discord::payload(
        &cfg.agent.discord,
        &message.id,
        draft()["answer"].as_str().unwrap(),
    )
    .unwrap();
    let response = json!({"id":"1001","channel_id":"123","author":{"id":"999","bot":true},"content":body["content"]});
    let mock = Mock::new(vec![
        (200, json!([message])),
        model(draft()),
        (200, response),
    ]);
    let mut app = runtime(cfg, Some(&mock));
    app.enqueue(
        "reply".into(),
        Work::Discord {
            channel: "123".into(),
            message: Box::new(mention()),
        },
        100,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert_eq!(app.state.recent[0].result.as_ref().unwrap()["sent"], true);
    assert_eq!(app.state.announcements.len(), 1);
    let requests = mock.finish();
    let sent =
        serde_json::from_str::<Value>(requests[2].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(
        sent["content"]
            .as_str()
            .unwrap()
            .starts_with("I'm the user's AI assistant")
    );
    assert_eq!(sent["allowed_mentions"]["parse"], json!([]));
    assert_eq!(sent["enforce_nonce"], true);
}
#[test]
fn uncertain_discord_delivery_is_not_reposted_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let cfg = discord_config(root.path());
    let mock = Mock::new(vec![
        (200, json!([mention()])),
        model(draft()),
        (500, json!({"error":"lost response"})),
        (200, json!([])),
    ]);
    let mut app = runtime(cfg, Some(&mock));
    app.enqueue(
        "uncertain".into(),
        Work::Discord {
            channel: "123".into(),
            message: Box::new(mention()),
        },
        100,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert!(app.state.jobs[0].action.is_some());
    drop(app);
    let mut app = runtime(discord_config(root.path()), Some(&mock));
    app.tick(200).unwrap();
    assert_eq!(app.state.jobs[0].status, Status::Review);
    assert!(
        app.state
            .announcements
            .iter()
            .all(|a| !a.text.contains("answered a question"))
    );
    assert!(
        app.command(
            Command::Retry {
                job_id: "uncertain".into()
            },
            201
        )
        .is_err()
    );
    let requests = mock.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("POST /channels/"))
            .count(),
        1
    );
}
#[test]
fn present_user_and_bot_messages_never_trigger_replies() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = discord_config(root.path());
    cfg.agent.discord.away = false;
    let mut message = mention();
    assert!(discord::eligible(&message, &cfg.agent.discord));
    message.author.bot = true;
    assert!(!discord::eligible(&message, &cfg.agent.discord));
    let mut app = runtime(cfg, None);
    app.enqueue(
        "present".into(),
        Work::Discord {
            channel: "123".into(),
            message: Box::new(mention()),
        },
        100,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert_eq!(app.state.model_calls, 0);
    assert_eq!(app.state.recent[0].result.as_ref().unwrap()["sent"], false);
}
#[test]
fn paused_agent_keeps_jobs_and_voice_controls_are_typed() {
    let root = tempfile::tempdir().unwrap();
    let mut app = runtime(config(root.path()), None);
    app.command(Command::Pause, 100).unwrap();
    app.enqueue(
        "waiting".into(),
        Work::Ask {
            text: "hello".into(),
        },
        100,
    )
    .unwrap();
    app.tick(101).unwrap();
    assert_eq!(app.state.jobs[0].attempts, 0);
    assert!(voice_command("hello", "Lion").is_none());
    assert!(matches!(
        voice_command("Lion, pause!", "Lion"),
        Some(Command::Pause)
    ));
    assert!(matches!(
        voice_command("Lion undo that", "Lion"),
        Some(Command::UndoLast)
    ));
    assert!(matches!(
        voice_command("rm -rf /", ""),
        Some(Command::Ask { .. })
    ));
}
#[cfg(unix)]
#[test]
fn daemon_control_remains_available_and_shutdown_persists_preferences() {
    use std::process::{Command as Process, Stdio};
    let root = tempfile::tempdir().unwrap();
    let text = fs::read_to_string("config/system.toml").unwrap().replace(
        "data_dir = \"data\"",
        &format!("data_dir = {:?}", root.path().join("data")),
    );
    // system.toml may contain an [agent] section; append control_dir immediately under it.
    let text = if text.contains("[agent]\n") {
        text.replace(
            "[agent]\n",
            &format!("[agent]\ncontrol_dir = {:?}\n", root.path().join("control")),
        )
    } else {
        format!(
            "{text}\n[agent]\ncontrol_dir = {:?}\n",
            root.path().join("control")
        )
    };
    let path = root.path().join("config.toml");
    fs::write(&path, text).unwrap();
    let cfg = Config::load(&path).unwrap();
    let mut child = Process::new(env!("CARGO_BIN_EXE_lion-assist"))
        .args(["--config", path.to_str().unwrap(), "daemon"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while daemon::send(&cfg, Command::Status).is_err() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        daemon::send(&cfg, Command::Away { enabled: true }).unwrap()["accepted"],
        true
    );
    daemon::send(&cfg, Command::Pause).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        daemon::send(&cfg, Command::Status).unwrap()["paused_requested"],
        true
    );
    daemon::send(&cfg, Command::Stop).unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            panic!("daemon failed to stop");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(child.wait().unwrap().success());
    let app = Runtime::new(cfg).unwrap();
    assert!(app.state.away && app.state.paused);
}

#[test]
fn discord_backfill_keeps_cursor_when_queue_is_full_and_continues_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = discord_config(root.path());
    cfg.agent.max_jobs = 1;
    let mut messages = Vec::new();
    for id in 1001..=1100 {
        let mut message = mention();
        message.id = id.to_string();
        messages.push(message);
    }
    let second = messages
        .iter()
        .filter(|m| m.id.parse::<u64>().unwrap() < 1100)
        .cloned()
        .collect::<Vec<_>>();
    let mock = Mock::new(vec![(200, json!(messages)), (200, json!(second))]);
    let mut app = runtime(cfg, Some(&mock));
    app.state
        .discord_cursors
        .insert("123".into(), "1000".into());
    app.state.next_poll = 0;
    // Backpressure must not advance a channel cursor.
    app.enqueue(
        "busy".into(),
        Work::Ask {
            text: "later".into(),
        },
        999999,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert_eq!(app.state.discord_cursors["123"], "1000");
    app.state.jobs.clear();
    app.poll_sources(101).unwrap();
    assert_eq!(app.state.jobs.len(), 1);
    assert_eq!(app.state.discord_cursors["123"], "1000");
    assert_eq!(app.state.discord_backfill["123"].1, "1100");
    app.command(
        Command::Dismiss {
            job_id: app.state.jobs[0].id.clone(),
        },
        101,
    )
    .unwrap();
    drop(app);
    let mut cfg = discord_config(root.path());
    cfg.agent.max_jobs = 1;
    let mut app = runtime(cfg, Some(&mock));
    app.poll_sources(102).unwrap();
    assert_eq!(app.state.jobs.len(), 1);
    assert_eq!(app.state.discord_backfill["123"].1, "1099");
    let requests = mock.finish();
    assert!(requests[1].contains("before=1100"));
}
#[test]
fn discord_rate_limit_delays_retry_without_leaving_uncertain_send_marker() {
    let root = tempfile::tempdir().unwrap();
    let cfg = discord_config(root.path());
    let mock = Mock::new(vec![
        (200, json!([mention()])),
        model(draft()),
        (429, json!({"retry_after":120.5})),
    ]);
    let mut app = runtime(cfg, Some(&mock));
    app.enqueue(
        "limited".into(),
        Work::Discord {
            channel: "123".into(),
            message: Box::new(mention()),
        },
        100,
    )
    .unwrap();
    app.tick(100).unwrap();
    assert_eq!(app.state.jobs[0].next_attempt, 221);
    assert!(app.state.jobs[0].action.is_none());
    assert!(app.state.jobs[0].proposal.is_some());
    app.tick(220).unwrap();
    assert_eq!(app.state.jobs[0].attempts, 1);
    assert_eq!(mock.finish().len(), 3);
}
#[test]
fn quiet_hours_defer_normal_speech_but_allow_critical_events() {
    use lion_assist::agent::state::Announcement;
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.voice.enabled = true;
    cfg.agent.voice.quiet_start_utc = Some(22);
    cfg.agent.voice.quiet_end_utc = Some(7);
    let mut app = runtime(cfg, None);
    app.state.announcements.push(Announcement {
        id: "paper".into(),
        text: "paper".into(),
        detail: "details".into(),
        urgent: false,
    });
    app.state.announcements.push(Announcement {
        id: "critical".into(),
        text: "email".into(),
        detail: "details".into(),
        urgent: true,
    });
    assert_eq!(
        app.take_announcement(23 * 3600).unwrap().unwrap().id,
        "critical"
    );
    assert!(app.take_announcement(23 * 3600).unwrap().is_none());
    assert_eq!(
        app.take_announcement(8 * 3600).unwrap().unwrap().id,
        "paper"
    );
    assert!(app.take_announcement(8 * 3600).unwrap().is_none());
}
#[test]
fn missed_rundowns_coalesce_and_budget_exhaustion_does_not_spin() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.agent.research_queries = vec!["ion propulsion".into()];
    let mut app = runtime(cfg, None);
    app.state.next_rundown = 0;
    app.state.next_research = 100;
    // Queue one due research task without invoking its network step.
    app.schedule(100000).unwrap();
    assert_eq!(app.state.jobs.len(), 1);
    assert_eq!(app.state.next_research, 121600);
    app.schedule(100001).unwrap();
    assert_eq!(app.state.jobs.len(), 1);
}
