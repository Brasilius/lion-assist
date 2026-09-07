use super::audio::run;
use crate::core::config::Voice;
use anyhow::{Result, ensure};
pub fn speak(config: &Voice, text: &str) -> Result<()> {
    ensure!(config.enabled, "voice is disabled; set voice.enabled=true");
    run(&config.speak_command, text, config.timeout_secs)?;
    Ok(())
}
pub fn listen(config: &Voice) -> Result<String> {
    ensure!(config.enabled, "voice is disabled; set voice.enabled=true");
    let text = run(&config.listen_command, "", config.timeout_secs)?;
    ensure!(
        !text.is_empty(),
        "speech recognizer returned an empty transcript"
    );
    Ok(text)
}
