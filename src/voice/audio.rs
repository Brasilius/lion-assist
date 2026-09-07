use crate::core::limits::read_bounded;
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
/// Trusted local adapters only. Arguments are never interpreted by a shell.
/// Adapters must keep their children in the inherited process group.
pub fn run(argv: &[String], input: &str, timeout_secs: u64) -> Result<String> {
    ensure!(!argv.is_empty(), "voice command is not configured");
    ensure!(input.len() <= 4000, "speech text exceeds 4000 bytes");
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .context("cannot start voice adapter; install/configure it first")?;
    let id = child.id();
    let stdout = child.stdout.take().context("voice stdout unavailable")?;
    let reader = std::thread::spawn(move || read_bounded(stdout, 16_384));
    let mut stdin = child.stdin.take().context("voice stdin unavailable")?;
    let input = input.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let start = Instant::now();
    let result = loop {
        if let Some(status) = child.try_wait()? {
            break Ok(status);
        }
        if start.elapsed() >= Duration::from_secs(timeout_secs) {
            break Err(anyhow::anyhow!("voice adapter timed out"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    #[cfg(unix)]
    {
        use nix::{
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        let _ = killpg(Pid::from_raw(id as i32), Signal::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let status = result?;
    ensure!(status.success(), "voice adapter failed: {status}");
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("voice writer panicked"))??;
    let data = reader
        .join()
        .map_err(|_| anyhow::anyhow!("voice reader panicked"))??;
    let text = String::from_utf8(data)?;
    if text.len() > 16_384 {
        bail!("voice transcript too large");
    }
    Ok(text.trim().into())
}
