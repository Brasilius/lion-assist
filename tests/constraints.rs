use lion_assist::{
    core::{
        config::Config,
        limits::{Store, read_bounded},
        models::{Complexity, Router},
    },
    os::monitor::Http,
    tasks::{
        aerospace::{
            compress::{compress, decompress},
            crawler::{parse_arxiv, parse_crossref},
            storage,
        },
        email::{self, Tier},
    },
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
};

#[test]
fn quota_rejects_before_write_and_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path(), 100_000, 10_000).unwrap();
    assert!(store.put("first", &[1; 20_000]).unwrap());
    let used = store.used().unwrap();
    assert!(!store.put("first", &[2; 20_000]).unwrap());
    assert_eq!(used, store.used().unwrap());
    assert!(store.put("too-large", &[0; 90_000]).is_err());
    assert!(!root.path().join("too-large").exists());
    assert!(Store::open(root.path(), 100_000, 10_000).is_err());
    drop(store);
    let mut restarted = Store::open(root.path(), 100_000, 10_000).unwrap();
    assert_eq!(used, restarted.used().unwrap());
    assert!(restarted.put("../escape", b"no").is_err());
    assert!(restarted.put(".lock", b"no").is_err());
    drop(restarted);
    assert!(Store::open(root.path(), 10_000, 1).is_err());
}
#[test]
fn orphaned_writes_are_counted() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".orphan.part"), [0; 30_000]).unwrap();
    assert!(Store::open(root.path(), 25_000, 1).is_err());
}
#[cfg(unix)]
#[test]
fn symlinks_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/tmp", root.path().join("escape")).unwrap();
    assert!(Store::open(root.path(), 100_000, 1).is_err());
}
#[test]
fn bounded_reads_and_decompression() {
    assert!(read_bounded(&b"12345"[..], 4).is_err());
    assert_eq!(read_bounded(&b"1234"[..], 4).unwrap(), b"1234");
    let original = vec![b'a'; 100_000];
    let packed = compress(&original).unwrap();
    assert!(packed.len() < 1000);
    assert_eq!(decompress(&packed, 100_000).unwrap(), original);
    assert!(decompress(&packed, 1000).is_err());
}
#[test]
fn email_tier_precedence_and_idempotence() {
    for (subject, body, expected) in [
        ("Bank statement", "receipt newsletter", Tier::Critical),
        ("A newsletter", "Your tax document is ready", Tier::Critical),
        ("Your verification code", "unsubscribe", Tier::Important),
        ("Receipt for your order", "", Tier::Important),
        ("Aerospace news", "", Tier::News),
        ("Hello", "Lunch tomorrow?", Tier::Other),
    ] {
        assert_eq!(email::classify(subject, body, false).0, expected);
    }
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(&root.path().join("data"), 1_000_000, 10_000).unwrap();
    let message = root.path().join("test.eml");
    fs::write(&message, "From: bank@example.test\r\nSubject: =?utf-8?Q?Bank_statement?=\r\nContent-Type: text/plain\r\n\r\nKeep this record.").unwrap();
    let (record, fresh) = email::organize(&message, &mut store, 10_000).unwrap();
    assert!(fresh);
    assert_eq!(record.tier, Tier::Critical);
    assert!(!email::organize(&message, &mut store, 10_000).unwrap().1);
    assert!(email::organize(&message, &mut store, 10).is_err());
}
#[test]
fn research_parsing_compression_and_deduplication() {
    let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>http://arxiv.org/abs/2601.01234v1</id><title>Spacecraft
    propulsion</title><summary>An aerospace abstract.</summary><published>2026-01-01</published></entry></feed>"#;
    let papers = parse_arxiv(xml).unwrap();
    assert_eq!(papers[0].title, "Spacecraft propulsion");
    assert!(papers[0].url.starts_with("https://"));
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(root.path(), 1_000_000, 10_000).unwrap();
    assert!(storage::save(&mut store, &papers[0]).unwrap());
    assert!(!storage::save(&mut store, &papers[0]).unwrap());
    let objects = store.list("paper-", 10).unwrap();
    let data = decompress(&store.get(&objects[0], 10_000).unwrap(), 10_000).unwrap();
    assert!(
        String::from_utf8(data)
            .unwrap()
            .contains("An aerospace abstract.")
    );
    let crossref = br#"{"message":{"items":[{"DOI":"10.123/a","title":["Aerodynamics"],"abstract":"Lift"},{"DOI":"missing-title"}]}}"#;
    assert_eq!(parse_crossref(crossref).unwrap().len(), 1);
    assert!(parse_crossref(b"{}").is_err());
    assert!(parse_arxiv(b"<feed><entry>").is_err());
}
#[test]
fn config_caps_and_routing() {
    use lion_assist::core::models::complexity_for;
    assert!(matches!(complexity_for("Hello", false), Complexity::Simple));
    assert!(matches!(
        complexity_for("Compare propulsion systems", false),
        Complexity::Complex
    ));
    assert!(matches!(complexity_for("Hello", true), Complexity::Complex));
    let mut config = Config::load(std::path::Path::new("config/system.toml")).unwrap();
    let router = Router::new();
    assert_eq!(
        router.choose(&config, Complexity::Simple).model,
        config.models.cheap.model
    );
    assert_eq!(
        router.choose(&config, Complexity::Complex).model,
        config.models.advanced.model
    );
    config.limits.disk_bytes = 50_000_000_001;
    assert!(config.validate().is_err());
    config.limits.disk_bytes = 50_000_000_000;
    config.research.interval_secs = 0;
    assert!(config.validate().is_err());
}
#[test]
fn actual_model_http_request_and_call_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..pos]);
                let len: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        let request = String::from_utf8(bytes).unwrap();
        assert!(request.contains("/models/gemini-3-flash-preview:generateContent"));
        assert!(request.contains("maxOutputTokens"));
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"A short summary."}]}}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let mut config = Config::load(std::path::Path::new("config/system.toml")).unwrap();
    config.models.cheap.base_url = base;
    config.models.cheap.api_key_env = "PATH".into(); // Existing non-secret env var; no process-global mutation.
    config.limits.max_model_calls_per_run = 1;
    let http = Http::new(&config.limits).unwrap();
    let mut router = Router::new();
    assert_eq!(
        router
            .ask(&config, &http, Complexity::Simple, "Summarize", "hello")
            .unwrap(),
        "A short summary."
    );
    assert!(
        router
            .ask(&config, &http, Complexity::Simple, "Summarize", "hello")
            .unwrap_err()
            .to_string()
            .contains("budget")
    );
    server.join().unwrap();
}
#[test]
fn event_loop_recovers_from_invalid_input_and_quits() {
    let root = tempfile::tempdir().unwrap();
    let config = fs::read_to_string("config/system.toml").unwrap().replace(
        "data_dir = \"data\"",
        &format!("data_dir = {:?}", root.path().join("data")),
    );
    let path = root.path().join("config.toml");
    fs::write(&path, config).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lion-assist"))
        .args(["--config", path.to_str().unwrap(), "run"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"invalid\n{\"type\":\"status\"}\n{\"type\":\"quit\"}\n{\"type\":\"status\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("error"));
    assert!(lines[1].contains("used_bytes"));
    assert!(lines[2].contains("stopped"));
}
#[cfg(unix)]
#[test]
fn voice_adapter_round_trip_and_timeout() {
    use lion_assist::voice::audio::run;
    assert_eq!(run(&["cat".into()], "hello", 2).unwrap(), "hello");
    let start = std::time::Instant::now();
    assert!(run(&["sleep".into(), "10".into()], "", 1).is_err());
    assert!(start.elapsed().as_secs() < 3);
}

#[test]
fn mailbox_cursor_makes_progress_past_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let maildir = root.path().join("mail");
    fs::create_dir_all(maildir.join("new")).unwrap();
    for i in 0..3 {
        fs::write(
            maildir.join("new").join(i.to_string()),
            format!("Subject: Receipt {i}\n\nThank you."),
        )
        .unwrap();
    }
    let mut store = Store::open(&root.path().join("data"), 1_000_000, 10_000).unwrap();
    let mut scanner = email::MaildirScanner::new(&maildir);
    let mut count = 0;
    for _ in 0..4 {
        count += scanner.scan(&mut store, 10_000, 1).unwrap().len();
    }
    assert_eq!(count, 3);
    for _ in 0..4 {
        assert!(scanner.scan(&mut store, 10_000, 1).unwrap().is_empty());
    }
}

#[test]
fn event_loop_discards_oversized_line_and_recovers() {
    let root = tempfile::tempdir().unwrap();
    let config = fs::read_to_string("config/system.toml")
        .unwrap()
        .replace(
            "data_dir = \"data\"",
            &format!("data_dir = {:?}", root.path().join("data")),
        )
        .replace("max_input_bytes = 8388608", "max_input_bytes = 64");
    let path = root.path().join("config.toml");
    fs::write(&path, config).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lion-assist"))
        .args(["--config", path.to_str().unwrap(), "run"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut events = vec![b'x'; 128];
    events.extend_from_slice(b"\n{\"type\":\"status\"}\n");
    child.stdin.take().unwrap().write_all(&events).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains("event exceeds input limit"));
    assert!(text.contains("used_bytes"));
}
