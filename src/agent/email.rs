use crate::{
    core::limits::{digest, read_file},
    tasks::email::{self, Tier},
};
use anyhow::{Context, Result, ensure};
use mailparse::MailHeaderMap;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct Evidence {
    pub from: String,
    pub subject: String,
    pub body: String,
    pub heuristic: Tier,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub tier: Tier,
    pub reason: String,
    pub evidence: Vec<String>,
    pub needs_review: bool,
}
impl Decision {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.reason.trim().is_empty() && self.reason.len() <= 2000,
            "invalid email reason"
        );
        ensure!(
            !self.evidence.is_empty()
                && self.evidence.len() <= 8
                && self
                    .evidence
                    .iter()
                    .all(|x| !x.is_empty() && x.len() <= 1000),
            "invalid email evidence"
        );
        Ok(())
    }
}
pub fn evidence(path: &Path, expected: &str, limit: u64) -> Result<Evidence> {
    let bytes = read_file(path, limit)?;
    ensure!(digest(&bytes) == expected, "email changed since discovery");
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
    email::text_parts(&mail, &mut body, &mut 24_000)?;
    let heuristic = email::classify(
        &subject,
        &body,
        mail.headers.get_first_value("List-Id").is_some(),
    )
    .0;
    Ok(Evidence {
        from,
        subject,
        body,
        heuristic,
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub hash: String,
}
fn checked_root(root: &Path) -> Result<PathBuf> {
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "Maildir root cannot be a symlink"
    );
    Ok(root.canonicalize()?)
}
fn safe_parent(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .context("mail path outside configured Maildir")?;
    ensure!(
        relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "invalid mail path"
    );
    let mut current = root.to_path_buf();
    for component in relative
        .parent()
        .context("missing mail parent")?
        .components()
    {
        current.push(component);
        let meta = fs::symlink_metadata(&current)?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "Maildir parent must be a real directory"
        );
    }
    Ok(())
}
pub fn plan(root: &Path, source: &Path, hash: &str, tier: Tier) -> Result<Move> {
    let root = checked_root(root)?;
    let source = source.to_path_buf();
    safe_parent(&root, &source)?;
    let folder = match tier {
        Tier::Critical => "Critical",
        Tier::Important => "Important",
        Tier::News => "News",
        Tier::Other => "Other",
    };
    let parent = root.join(format!(".Lion-{folder}"));
    if !parent.exists() {
        fs::create_dir(&parent)?;
    }
    ensure!(
        fs::symlink_metadata(&parent)?.file_type().is_dir(),
        "invalid destination folder"
    );
    for name in ["new", "cur", "tmp"] {
        let path = parent.join(name);
        if !path.exists() {
            fs::create_dir(&path)?;
        }
        ensure!(
            fs::symlink_metadata(path)?.file_type().is_dir(),
            "invalid Maildir folder"
        );
    }
    // Preserve Maildir flags, with a content-derived name that cannot collide with another message.
    let flags = source
        .file_name()
        .and_then(|x| x.to_str())
        .and_then(|x| x.split_once(":2,"))
        .map(|(_, flags)| flags)
        .unwrap_or("")
        .to_owned();
    ensure!(
        flags.chars().all(|x| x.is_ascii_alphabetic()),
        "invalid Maildir flags"
    );
    Ok(Move {
        source,
        destination: parent.join("cur").join(format!("lion-{hash}:2,{flags}")),
        hash: hash.into(),
    })
}
/// Hard-link then unlink: never overwrite an existing message; recover both crash windows.
pub fn apply(root: &Path, action: &Move, limit: u64) -> Result<()> {
    let root = checked_root(root)?;
    safe_parent(&root, &action.source)?;
    safe_parent(&root, &action.destination)?;
    if action.destination.exists() {
        ensure!(
            digest(&read_file(&action.destination, limit)?) == action.hash,
            "destination conflict"
        );
    } else {
        ensure!(
            digest(&read_file(&action.source, limit)?) == action.hash,
            "source changed or missing"
        );
        fs::hard_link(&action.source, &action.destination)?;
        fs::File::open(action.destination.parent().unwrap())?.sync_all()?;
    }
    if action.source.exists() {
        ensure!(
            digest(&read_file(&action.source, limit)?) == action.hash,
            "source changed after move"
        );
        fs::remove_file(&action.source)?;
        fs::File::open(action.source.parent().unwrap())?.sync_all()?;
    }
    ensure!(
        digest(&read_file(&action.destination, limit)?) == action.hash,
        "move verification failed"
    );
    Ok(())
}
