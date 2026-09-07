use crate::{os::monitor::Http, tasks::aerospace::storage::Paper};
use anyhow::{Result, ensure};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

pub fn text(html: &str, max: usize) -> String {
    let document = Html::parse_fragment(html);
    let mut output = String::new();
    for node in document.tree.nodes() {
        if let scraper::node::Node::Text(value) = node.value() {
            let hidden = node.ancestors().any(|n| {
                n.value().as_element().is_some_and(|e| {
                    matches!(e.name(), "script" | "style" | "nav" | "footer" | "noscript")
                })
            });
            if hidden {
                continue;
            }
            for word in value.split_whitespace() {
                if output.len() + word.len() + 1 > max {
                    return output;
                }
                if !output.is_empty() {
                    output.push(' ');
                }
                output.push_str(word);
            }
        }
    }
    output
}
#[derive(Default, Deserialize)]
struct Rss {
    #[serde(default)]
    channel: Channel,
}
#[derive(Default, Deserialize)]
struct Channel {
    #[serde(default)]
    item: Vec<Item>,
}
#[derive(Default, Deserialize)]
struct Item {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "pubDate")]
    date: String,
}
#[derive(Default, Deserialize)]
struct Atom {
    #[serde(default)]
    entry: Vec<Entry>,
}
#[derive(Default, Deserialize)]
struct Entry {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: Vec<Link>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    published: String,
}
#[derive(Default, Deserialize)]
struct Link {
    #[serde(default, rename = "@href")]
    href: String,
    #[serde(default, rename = "@rel")]
    rel: String,
}
/// Configured RSS/Atom sources or a single article page. No unrestricted model-selected URLs.
pub fn parse(url: &str, bytes: &[u8], max: usize) -> Result<Vec<Paper>> {
    let content = std::str::from_utf8(bytes)?;
    let mut entries = Vec::new();
    if let Ok(rss) = quick_xml::de::from_str::<Rss>(content) {
        entries.extend(
            rss.channel
                .item
                .into_iter()
                .map(|i| (i.title, i.link, i.description, i.date)),
        );
    }
    if entries.is_empty()
        && let Ok(atom) = quick_xml::de::from_str::<Atom>(content)
    {
        entries.extend(atom.entry.into_iter().map(|e| {
            (
                e.title,
                e.link
                    .into_iter()
                    .find(|l| l.rel.is_empty() || l.rel == "alternate")
                    .map(|l| l.href)
                    .unwrap_or_default(),
                e.summary,
                e.published,
            )
        }));
    }
    if entries.is_empty() {
        let document = Html::parse_document(content);
        let title = document
            .select(&Selector::parse("title").unwrap())
            .next()
            .map(|x| x.text().collect::<String>())
            .unwrap_or_default();
        ensure!(
            !title.trim().is_empty(),
            "source is neither a supported feed nor an article page"
        );
        let article = document
            .select(&Selector::parse("article, main").unwrap())
            .next()
            .map(|x| x.html())
            .unwrap_or_else(|| content.into());
        entries.push((title, url.into(), text(&article, 16000), String::new()));
    }
    let base = reqwest::Url::parse(url)?;
    Ok(entries
        .into_iter()
        .take(max)
        .filter_map(|(title, link, body, date)| {
            let mut link = base.join(&link).ok()?;
            if link.scheme() != "https"
                && !(link.scheme() == "http"
                    && matches!(link.host_str(), Some("localhost" | "127.0.0.1")))
            {
                return None;
            }
            if !link.username().is_empty() || link.password().is_some() || title.trim().is_empty() {
                return None;
            }
            link.set_fragment(None);
            Some(Paper {
                source: "web".into(),
                id: link.to_string(),
                url: link.to_string(),
                title: text(&title, 1000),
                abstract_text: text(&body, 16000),
                published: date,
            })
        })
        .collect())
}
pub fn fetch(http: &Http, url: &str, max: usize) -> Result<Vec<Paper>> {
    super::config::endpoint(url)?;
    parse(url, &http.send(http.client.get(url))?, max)
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub relevant: bool,
    pub needs_review: bool,
    pub summary: String,
    pub reason: String,
}
impl Selection {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.reason.trim().is_empty()
                && self.reason.len() <= 2000
                && self.summary.len() <= 3000,
            "invalid research selection"
        );
        ensure!(
            !self.relevant || !self.summary.trim().is_empty(),
            "selected research requires a summary"
        );
        Ok(())
    }
}
