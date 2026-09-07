use crate::core::limits::{Store, digest, read_file};
use anyhow::{Result, ensure};
use mailparse::MailHeaderMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Critical = 1,
    Important = 2,
    News = 3,
    Other = 4,
}
impl Tier {
    pub fn number(self) -> u8 {
        self as u8
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct OrganizedEmail {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub tier: Tier,
    pub reason: String,
}
const CRITICAL: &[&str] = &[
    "bank statement",
    "financial statement",
    "financial record",
    "tax return",
    "tax document",
    "w-2",
    "1099",
    "fraud alert",
    "security breach",
    "account compromised",
    "legal notice",
    "court summons",
    "medical emergency",
    "evacuation",
    "mortgage statement",
    "investment statement",
    "urgent action required",
];
const IMPORTANT: &[&str] = &[
    "verification code",
    "security code",
    "one-time",
    "authentication",
    "sign-in",
    "sign in",
    "login",
    "password reset",
    "receipt",
    "invoice",
    "order confirmation",
    "payment confirmation",
    "appointment",
    "boarding pass",
    "verify your email",
];
const NEWS: &[&str] = &[
    "newsletter",
    "digest",
    "aerospace news",
    "tech news",
    "technology news",
    "space news",
    "weekly roundup",
    "unsubscribe",
];
fn matching<'a>(text: &str, terms: &'a [&str]) -> Option<&'a str> {
    terms.iter().copied().find(|term| text.contains(term))
}
pub fn classify(subject: &str, body: &str, newsletter: bool) -> (Tier, String) {
    let text = format!("{subject}\n{body}").to_lowercase();
    for (tier, terms) in [
        (Tier::Critical, CRITICAL),
        (Tier::Important, IMPORTANT),
        (Tier::News, NEWS),
    ] {
        if let Some(term) = matching(&text, terms) {
            return (tier, format!("matched phrase: {term}"));
        }
    }
    if newsletter {
        return (Tier::News, "List-Id or List-Unsubscribe header".into());
    }
    (
        Tier::Other,
        "no higher-priority rule matched; review if needed".into(),
    )
}
pub(crate) fn text_parts(
    mail: &mailparse::ParsedMail<'_>,
    out: &mut String,
    remaining: &mut usize,
) -> Result<()> {
    if *remaining == 0 {
        return Ok(());
    }
    if mail.subparts.is_empty()
        && mail.ctype.mimetype.starts_with("text/")
        && mail.get_content_disposition().disposition != mailparse::DispositionType::Attachment
    {
        let body = mail.get_body()?;
        let end = body
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= *remaining)
            .last()
            .unwrap_or(0);
        let end = if body.len() <= *remaining {
            body.len()
        } else {
            end
        };
        out.push_str(&body[..end]);
        *remaining -= end;
    }
    for part in &mail.subparts {
        text_parts(part, out, remaining)?;
    }
    Ok(())
}
pub fn organize(path: &Path, store: &mut Store, max_bytes: u64) -> Result<(OrganizedEmail, bool)> {
    let bytes = read_file(path, max_bytes)?;
    let id = digest(&bytes);
    let name = format!("email-{id}.json");
    if store.contains(&name)? {
        return Ok((
            serde_json::from_slice(&store.get(&name, max_bytes)?)?,
            false,
        ));
    }
    // Bound MIME nesting before the recursive MIME parser sees the message.
    ensure!(
        bytes
            .windows(8)
            .filter(|w| w.eq_ignore_ascii_case(b"boundary"))
            .count()
            <= 32,
        "too many MIME boundaries"
    );
    let mail = mailparse::parse_mail(&bytes)?;
    let subject = mail.headers.get_first_value("Subject").unwrap_or_default();
    let from = mail.headers.get_first_value("From").unwrap_or_default();
    let mut body = String::new();
    text_parts(&mail, &mut body, &mut 128_000)?;
    let (tier, reason) = classify(
        &subject,
        &body,
        mail.headers.get_first_value("List-Id").is_some()
            || mail.headers.get_first_value("List-Unsubscribe").is_some(),
    );
    let record = OrganizedEmail {
        id,
        from,
        subject,
        tier,
        reason,
    };
    // Persist classifications only; originals and attachments stay in the mailbox.
    let fresh = store.put(&name, &serde_json::to_vec(&record)?)?;
    Ok((record, fresh))
}
/// Retain a directory cursor between ticks, bounding both work and memory.
pub struct MaildirScanner {
    root: std::path::PathBuf,
    current: Option<std::fs::ReadDir>,
    directory: usize,
}
impl MaildirScanner {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            current: None,
            directory: 0,
        }
    }
    pub fn scan(
        &mut self,
        store: &mut Store,
        max_bytes: u64,
        batch: usize,
    ) -> Result<Vec<OrganizedEmail>> {
        let mut output = Vec::new();
        let mut examined = 0;
        while examined < batch {
            if self.current.is_none() {
                if self.directory == 2 {
                    self.directory = 0;
                    break;
                }
                let path = self.root.join(["new", "cur"][self.directory]);
                self.directory += 1;
                if !path.exists() {
                    continue;
                }
                self.current = Some(std::fs::read_dir(path)?);
            }
            let entry = self.current.as_mut().and_then(Iterator::next);
            let Some(entry) = entry else {
                self.current = None;
                continue;
            };
            examined += 1;
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            match organize(&entry.path(), store, max_bytes) {
                Ok((record, true)) => output.push(record),
                Ok((_, false)) => {}
                Err(error) => eprintln!("email skipped: {error:#}"),
            }
        }
        Ok(output)
    }
}
