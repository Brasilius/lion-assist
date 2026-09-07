use super::compress::compress;
use crate::core::limits::{Store, digest};
use anyhow::Result;
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
pub struct Paper {
    pub source: String,
    pub id: String,
    pub title: String,
    pub abstract_text: String,
    pub url: String,
    pub published: String,
}
pub fn save(store: &mut Store, paper: &Paper) -> Result<bool> {
    let name = format!(
        "paper-{}.json.gz",
        digest(format!("{}:{}", paper.source, paper.id).as_bytes())
    );
    if store.contains(&name)? {
        return Ok(false);
    }
    store.put(&name, &compress(&serde_json::to_vec(paper)?)?)
}
