use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use lion_assist::{
    core::{config::Config, controller::Controller, limits::read_file},
    tasks::{aerospace::compress::decompress, email, task::Task},
};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(version, about = "Lion: a bounded personal assistant")]
struct Cli {
    #[arg(long, default_value = "config/system.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Check local setup without contacting providers or revealing credentials.
    Doctor,
    /// Run the persistent voice agent with scheduled workflows and durable state.
    Daemon {
        /// Perform a bounded batch, print status, and exit.
        #[arg(long)]
        once: bool,
    },
    /// Send a JSON control command or a natural-language voice-style request.
    Agent {
        /// Examples: status, pause, "research ion propulsion", or '{"command":"away","enabled":true}'.
        text: String,
    },
    /// Show disk usage and enforcement boundary.
    Status,
    /// Process one JSON event per line until EOF or quit.
    Run,
    Ask {
        text: String,
        #[arg(long)]
        complex: bool,
    },
    Email {
        path: PathBuf,
    },
    /// Poll a locally synchronized Maildir, persisting local tier labels.
    WatchEmail {
        maildir: PathBuf,
        #[arg(long)]
        once: bool,
    },
    /// Search arXiv and Crossref; gzip available abstracts and metadata.
    Research {
        query: String,
    },
    /// Summarize the latest 100 messages using a Discord bot token.
    Discord {
        channel: String,
    },
    /// Summarize a UTF-8 exported conversation using the cheap model.
    Summarize {
        path: PathBuf,
    },
    Speak {
        text: String,
    },
    /// Invoke the configured on-demand speech recognizer, answer and speak.
    Listen,
    /// List up to 1000 stored email or paper object names.
    List {
        #[arg(default_value = "email-")]
        prefix: String,
    },
    /// Print an object; gzip paper JSON is decompressed with a size bound.
    Show {
        name: String,
    },
}
fn print(value: &serde_json::Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    match cli.command {
        Action::Doctor => return print(&lion_assist::agent::daemon::doctor(&config)),
        Action::Daemon { once } => return lion_assist::agent::daemon::run(config, once),
        Action::Agent { text } => {
            let command = if text.trim_start().starts_with('{') {
                serde_json::from_str(&text)?
            } else {
                lion_assist::agent::runtime::voice_command(&text, "")
                    .ok_or_else(|| anyhow::anyhow!("unsupported control request"))?
            };
            return print(&lion_assist::agent::daemon::send(&config, command)?);
        }
        _ => {}
    }
    let mut app = Controller::new(config)?;
    let task = match cli.command {
        Action::Doctor | Action::Daemon { .. } | Action::Agent { .. } => unreachable!(),
        Action::Status => Task::Status,
        Action::Ask { text, complex } => Task::Ask { text, complex },
        Action::Email { path } => Task::Email { path },
        Action::Research { query } => Task::Research { query },
        Action::Discord { channel } => Task::Discord { channel },
        Action::Speak { text } => Task::Speak { text },
        Action::Listen => Task::Listen,
        Action::Run => {
            let mut input = io::stdin().lock();
            return run(&mut app, &mut input);
        }
        Action::WatchEmail { maildir, once } => {
            ensure!(
                maildir.join("new").is_dir() || maildir.join("cur").is_dir(),
                "expected a Maildir with new/ or cur/"
            );
            let mut scanner = email::MaildirScanner::new(&maildir);
            loop {
                let records = scanner.scan(
                    &mut app.store,
                    app.config.limits.max_input_bytes,
                    app.config.limits.max_emails_per_tick,
                )?;
                for record in records {
                    app.announce(&record);
                    print(&serde_json::to_value(record)?)?;
                }
                if once {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_secs(app.config.limits.poll_secs));
            }
        }
        Action::Summarize { path } => {
            let text = String::from_utf8(read_file(&path, app.config.limits.max_input_bytes)?)?;
            let summary = app.summary(&text)?;
            return print(&serde_json::json!({"summary":summary}));
        }
        Action::List { prefix } => {
            return print(&serde_json::json!({"objects":app.store.list(&prefix,1000)?}));
        }
        Action::Show { name } => {
            let mut bytes = app.store.get(&name, app.config.limits.max_input_bytes)?;
            if name.ends_with(".gz") {
                bytes = decompress(&bytes, app.config.limits.max_input_bytes)?;
            }
            return print(&serde_json::from_slice(&bytes)?);
        }
    };
    print(&app.handle(task)?)
}
fn run(app: &mut Controller, input: &mut impl BufRead) -> Result<()> {
    loop {
        let mut line = Vec::new();
        let mut over = false;
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let end = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(available.len());
            let newline = available[end - 1] == b'\n';
            if line.len() + end > app.config.limits.max_input_bytes as usize {
                over = true;
            }
            if !over {
                line.extend_from_slice(&available[..end]);
            }
            input.consume(end);
            if newline {
                break;
            }
        }
        if over {
            print(&serde_json::json!({"error":"event exceeds input limit"}))?;
            continue;
        }
        if line.is_empty() {
            return Ok(());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let task = serde_json::from_slice::<Task>(&line);
        let quit = matches!(task, Ok(Task::Quit));
        let result = task
            .map_err(anyhow::Error::from)
            .and_then(|task| app.handle(task));
        match result {
            Ok(value) => print(&value)?,
            Err(error) => print(&serde_json::json!({"error":format!("{error:#}")}))?,
        }
        if quit {
            return Ok(());
        }
    }
}
