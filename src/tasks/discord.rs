use crate::os::monitor::Http;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
#[derive(Deserialize)]
struct Message {
    content: String,
    author: Author,
    timestamp: String,
}
#[derive(Deserialize)]
struct Author {
    username: String,
}
pub fn conversation(http: &Http, channel: &str, max_prompt: usize) -> Result<String> {
    ensure!(
        !channel.is_empty() && channel.len() <= 20 && channel.bytes().all(|c| c.is_ascii_digit()),
        "invalid Discord channel ID"
    );
    let token = std::env::var("DISCORD_BOT_TOKEN")
        .context("set DISCORD_BOT_TOKEN; personal user tokens are unsupported")?;
    let bytes = http.send(
        http.client
            .get(format!(
                "https://discord.com/api/v10/channels/{channel}/messages"
            ))
            .query(&[("limit", "100")])
            .header("Authorization", format!("Bot {token}")),
    )?;
    let messages: Vec<Message> = serde_json::from_slice(&bytes)?;
    ensure!(!messages.is_empty(), "channel has no messages");
    ensure!(
        messages.iter().any(|m| !m.content.is_empty()),
        "no readable message content; check bot permissions and Message Content intent"
    );
    let mut text = String::new();
    for message in messages.into_iter().rev() {
        let line = format!(
            "{} {}: {}\n",
            message.timestamp, message.author.username, message.content
        );
        ensure!(
            text.len() + line.len() <= max_prompt,
            "conversation exceeds prompt budget; use a smaller exported transcript"
        );
        text.push_str(&line);
    }
    Ok(text)
}
