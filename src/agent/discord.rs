use super::config::DiscordConfig;
use crate::os::monitor::Http;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub bot: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub channel_id: String,
    pub author: Author,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub mentions: Vec<Author>,
    #[serde(default)]
    pub webhook_id: Option<String>,
    #[serde(default)]
    pub message_reference: Option<Value>,
    #[serde(default)]
    pub nonce: Option<Value>,
}
pub fn snowflake(value: &str) -> Result<u64> {
    ensure!(
        !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit()),
        "invalid Discord ID"
    );
    Ok(value.parse()?)
}
pub fn eligible(message: &Message, config: &DiscordConfig) -> bool {
    !message.author.bot
        && message.webhook_id.is_none()
        && message.author.id != config.bot_id
        && !message.content.trim().is_empty()
        && message.mentions.iter().any(|a| a.id == config.bot_id)
}
fn token(config: &DiscordConfig) -> Result<String> {
    let token = std::env::var(&config.token_env)
        .with_context(|| format!("set {} for Discord", config.token_env))?;
    ensure!(!token.trim().is_empty(), "Discord token is empty");
    Ok(format!("Bot {token}"))
}
fn channel_url(config: &DiscordConfig, channel: &str) -> Result<String> {
    snowflake(channel)?;
    ensure!(
        config.enabled && config.channels.iter().any(|c| c == channel),
        "Discord channel is not enabled"
    );
    Ok(format!(
        "{}/channels/{channel}/messages",
        config.api_base.trim_end_matches('/')
    ))
}
pub fn messages(
    http: &Http,
    config: &DiscordConfig,
    channel: &str,
    after: Option<&str>,
) -> Result<Vec<Message>> {
    let mut request = http
        .client
        .get(channel_url(config, channel)?)
        .header("Authorization", token(config)?)
        .query(&[("limit", "100")]);
    if let Some(after) = after {
        snowflake(after)?;
        request = request.query(&[("after", after)]);
    }
    let mut items: Vec<Message> = serde_json::from_slice(&http.send(request)?)?;
    for message in &items {
        snowflake(&message.id)?;
        ensure!(
            message.channel_id.is_empty() || message.channel_id == channel,
            "Discord returned wrong channel"
        );
    }
    items.sort_by_key(|m| m.id.parse::<u64>().unwrap_or(0));
    Ok(items)
}
pub fn before(
    http: &Http,
    config: &DiscordConfig,
    channel: &str,
    before: &str,
) -> Result<Vec<Message>> {
    snowflake(before)?;
    let request = http
        .client
        .get(channel_url(config, channel)?)
        .header("Authorization", token(config)?)
        .query(&[("limit", "100"), ("before", before)]);
    let items: Vec<Message> = serde_json::from_slice(&http.send(request)?)?;
    for message in &items {
        snowflake(&message.id)?;
    }
    Ok(items)
}
pub fn context(
    http: &Http,
    config: &DiscordConfig,
    channel: &str,
    message: &str,
) -> Result<Vec<Message>> {
    snowflake(message)?;
    let request = http
        .client
        .get(channel_url(config, channel)?)
        .header("Authorization", token(config)?)
        .query(&[("limit", "20"), ("around", message)]);
    let mut items: Vec<Message> = serde_json::from_slice(&http.send(request)?)?;
    items.sort_by_key(|m| m.id.parse::<u64>().unwrap_or(0));
    Ok(items)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub answer: String,
    pub should_reply: bool,
    pub needs_review: bool,
    pub reason: String,
}
impl Draft {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.answer.len() <= 6000 && self.reason.len() <= 2000 && !self.reason.is_empty(),
            "invalid Discord draft"
        );
        ensure!(
            !self.should_reply || !self.answer.trim().is_empty(),
            "empty Discord answer"
        );
        Ok(())
    }
}
pub fn payload(config: &DiscordConfig, trigger: &str, answer: &str) -> Result<Value> {
    snowflake(trigger)?;
    let content = format!(
        "I'm {}'s AI assistant, replying while they're away.\n\n{}",
        config.owner_name,
        answer.trim()
    );
    ensure!(
        content.chars().count() <= 2000,
        "Discord reply exceeds 2000 characters"
    );
    Ok(
        json!({"content":content, "allowed_mentions":{"parse":[],"replied_user":false}, "message_reference":{"message_id":trigger,"fail_if_not_exists":true}, "nonce":trigger,"enforce_nonce":true}),
    )
}
pub fn send(http: &Http, config: &DiscordConfig, channel: &str, body: &Value) -> Result<Message> {
    ensure!(config.allow_send, "Discord sending is disabled");
    let response: Message = serde_json::from_slice(
        &http.send(
            http.client
                .post(channel_url(config, channel)?)
                .header("Authorization", token(config)?)
                .json(body),
        )?,
    )?;
    snowflake(&response.id)?;
    ensure!(
        response.author.id == config.bot_id
            && response.channel_id == channel
            && Some(response.content.as_str()) == body.get("content").and_then(Value::as_str),
        "Discord send response did not verify delivery"
    );
    Ok(response)
}
pub fn reconcile(
    http: &Http,
    config: &DiscordConfig,
    channel: &str,
    trigger: &str,
    body: &Value,
) -> Result<Option<Message>> {
    Ok(messages(http, config, channel, Some(trigger))?
        .into_iter()
        .find(|m| {
            m.author.id == config.bot_id
                && m.message_reference
                    .as_ref()
                    .and_then(|r| r.get("message_id"))
                    .and_then(Value::as_str)
                    == Some(trigger)
                && body.get("content").and_then(Value::as_str) == Some(m.content.as_str())
        }))
}
