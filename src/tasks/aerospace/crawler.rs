use super::storage::{Paper, save};
use crate::{
    core::{config::Config, limits::Store, scheduler::Pacer},
    os::monitor::Http,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Feed {
    #[serde(default)]
    entry: Vec<Entry>,
}
#[derive(Debug, Deserialize)]
struct Entry {
    id: String,
    title: String,
    summary: String,
    published: String,
}
pub fn parse_arxiv(bytes: &[u8]) -> Result<Vec<Paper>> {
    let feed: Feed = quick_xml::de::from_reader(bytes)?;
    Ok(feed
        .entry
        .into_iter()
        .map(|e| Paper {
            source: "arxiv".into(),
            id: e.id.clone(),
            url: e.id.replace("http://", "https://"),
            title: e.title.split_whitespace().collect::<Vec<_>>().join(" "),
            abstract_text: e.summary.trim().into(),
            published: e.published,
        })
        .collect())
}
pub fn parse_crossref(bytes: &[u8]) -> Result<Vec<Paper>> {
    let data: Value = serde_json::from_slice(bytes)?;
    let items = data
        .pointer("/message/items")
        .and_then(Value::as_array)
        .context("invalid Crossref response")?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let id = item.get("DOI")?.as_str()?.to_owned();
            Some(Paper {
                source: "crossref".into(),
                url: format!("https://doi.org/{id}"),
                id,
                title: item.pointer("/title/0")?.as_str()?.into(),
                abstract_text: item
                    .get("abstract")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                published: item
                    .pointer("/created/date-time")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            })
        })
        .collect())
}
/// Metadata and available abstracts, not paywalled full text or PDF mirrors.
pub fn crawl(
    config: &Config,
    http: &Http,
    store: &mut Store,
    pacer: &mut Pacer,
    query: &str,
) -> Result<usize> {
    ensure!(
        !query.trim().is_empty() && query.len() <= 512,
        "research query must be 1..=512 bytes"
    );
    let count = config.research.max_results.to_string();
    // Group the supplied query so API operators cannot bypass the aerospace constraint.
    let arxiv_query = format!(
        "(all:aerospace OR all:spacecraft OR all:aerodynamics OR all:propulsion) AND all:\"{}\"",
        query.replace(['"', '\\'], " ")
    );
    let crossref_query = format!("aerospace {query}");
    let requests = [
        http.client
            .get("https://export.arxiv.org/api/query")
            .query(&[
                ("search_query", arxiv_query.as_str()),
                ("max_results", count.as_str()),
                ("sortBy", "submittedDate"),
                ("sortOrder", "descending"),
            ]),
        http.client.get("https://api.crossref.org/works").query(&[
            ("query", crossref_query.as_str()),
            ("rows", count.as_str()),
            ("filter", "type:journal-article"),
        ]),
    ];
    let mut saved = 0;
    let mut succeeded = 0;
    for (index, request) in requests.into_iter().enumerate() {
        pacer.wait();
        let parsed = http.send(request).and_then(|bytes| {
            if index == 0 {
                parse_arxiv(&bytes)
            } else {
                parse_crossref(&bytes)
            }
        });
        match parsed {
            Ok(papers) => {
                succeeded += 1;
                for paper in papers.into_iter().take(config.research.max_results) {
                    if save(store, &paper)? {
                        saved += 1;
                    }
                    if config.research.download_pdfs && paper.source == "arxiv" {
                        archive_pdf(http, store, pacer, &paper)?;
                    }
                }
            }
            Err(error) => eprintln!(
                "research source {} failed: {error:#}",
                if index == 0 { "arXiv" } else { "Crossref" }
            ),
        }
    }
    ensure!(succeeded > 0, "all research sources failed");
    Ok(saved)
}

fn archive_pdf(http: &Http, store: &mut Store, pacer: &mut Pacer, paper: &Paper) -> Result<()> {
    use crate::core::limits::digest;
    let name = format!("pdf-{}.pdf.gz", digest(paper.id.as_bytes()));
    if store.contains(&name)? {
        return Ok(());
    }
    let id = paper.id.split_once("/abs/").context("invalid arXiv ID")?.1;
    ensure!(
        !id.is_empty()
            && !id.contains("..")
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-./".contains(c)),
        "invalid arXiv PDF identifier"
    );
    pacer.wait();
    let bytes = http.send(http.client.get(format!("https://arxiv.org/pdf/{id}")))?;
    ensure!(
        bytes.starts_with(b"%PDF-"),
        "arXiv returned a non-PDF response"
    );
    store.put(&name, &super::compress::compress(&bytes)?)?;
    Ok(())
}
