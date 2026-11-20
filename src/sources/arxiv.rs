//! Native arXiv Atom API adapter (`export.arxiv.org/api/query`) parsed directly in Rust.
//!
//! No external CLI required. Uses quoted phrase search sorted by relevance, falling back
//! to AND-joined terms when the exact phrase returns zero entries, and applies a 365-day
//! recency cutoff matching `last30days`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use quick_xml::Reader;
use quick_xml::events::Event;
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::{DateConfidence, DateWindow, Depth, Engagement, SourceId, SourceItem};

const ARXIV_API_URL: &str = "https://export.arxiv.org/api/query";
const RECENCY_DAYS: i64 = 365;

pub fn fetch(
    query: &str,
    window: DateWindow,
    depth: Depth,
    timeout: Duration,
    deadline: Instant,
) -> Result<(Vec<SourceItem>, Option<String>), HttpError> {
    let limit = match depth {
        Depth::Quick => 5,
        Depth::Default => 10,
        Depth::Deep => 20,
    };

    let quoted_q = build_search_query(query, true);
    let mut items = run_query(&quoted_q, limit, window.to, timeout, deadline)?;
    let mut mode = "atom/phrase";
    if items.is_empty() && query.split_whitespace().count() > 1 {
        let and_q = build_search_query(query, false);
        items = run_query(&and_q, limit, window.to, timeout, deadline)?;
        mode = "atom/and-fallback";
    }
    Ok((items, Some(mode.to_string())))
}

fn build_search_query(topic: &str, quoted: bool) -> String {
    let clean: String = topic
        .replace('"', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if quoted {
        format!("all:\"{clean}\"")
    } else {
        clean
            .split_whitespace()
            .map(|t| format!("all:\"{t}\""))
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}

fn run_query(
    search_query: &str,
    limit: usize,
    reference_date: NaiveDate,
    timeout: Duration,
    deadline: Instant,
) -> Result<Vec<SourceItem>, HttpError> {
    let mut url = Url::parse(ARXIV_API_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("search_query", search_query)
        .append_pair("sortBy", "relevance")
        .append_pair("sortOrder", "descending")
        .append_pair("max_results", &limit.to_string());

    let xml = http::get_text(url.as_str(), &[], timeout, deadline, 1)?;
    Ok(parse_atom_entries(&xml, reference_date))
}

fn parse_atom_entries(xml: &str, reference_date: NaiveDate) -> Vec<SourceItem> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut out = Vec::new();
    let mut in_entry = false;
    let mut current_tag = String::new();
    let mut id = String::new();
    let mut title = String::new();
    let mut summary = String::new();
    let mut published = String::new();
    let mut authors: Vec<String> = Vec::new();
    let mut in_author = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "entry" {
                    in_entry = true;
                    id.clear();
                    title.clear();
                    summary.clear();
                    published.clear();
                    authors.clear();
                } else if in_entry {
                    if name == "author" {
                        in_author = true;
                    }
                    current_tag = name;
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "author" {
                    in_author = false;
                } else if name == "entry" && in_entry {
                    in_entry = false;
                    if let Some(item) =
                        finish_entry(&id, &title, &summary, &published, &authors, reference_date)
                    {
                        out.push(item);
                    }
                }
                current_tag.clear();
            }
            Ok(Event::Text(e)) => {
                if !in_entry {
                    continue;
                }
                let text = e.unescape().map(|c| c.into_owned()).unwrap_or_default();
                match current_tag.as_str() {
                    "id" => id.push_str(&text),
                    "title" => {
                        if !title.is_empty() {
                            title.push(' ');
                        }
                        title.push_str(&text);
                    }
                    "summary" => {
                        if !summary.is_empty() {
                            summary.push(' ');
                        }
                        summary.push_str(&text);
                    }
                    "published" => published.push_str(&text),
                    "name" if in_author => authors.push(text),
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn finish_entry(
    id: &str,
    title: &str,
    summary: &str,
    published: &str,
    authors: &[String],
    reference_date: NaiveDate,
) -> Option<SourceItem> {
    let title_clean = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let summary_clean = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let url = id.trim().to_string();
    if title_clean.is_empty() || url.is_empty() {
        return None;
    }

    let pub_date = published
        .get(..10)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    if let Some(d) = pub_date {
        let age = (reference_date - d).num_days();
        if age > RECENCY_DAYS {
            return None;
        }
    }

    let item_id = url.rsplit('/').next().unwrap_or(&url).to_string();
    let author = (!authors.is_empty()).then(|| {
        authors
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    });

    Some(SourceItem {
        item_id,
        source: SourceId::Arxiv,
        title: title_clean,
        body: summary_clean.clone(),
        url,
        author,
        container: Some("arxiv.org".to_string()),
        published_at: pub_date.map(|d| d.format("%Y-%m-%d").to_string()),
        date_confidence: DateConfidence::High,
        engagement: Engagement::default(),
        snippet: summary_clean.chars().take(240).collect(),
        metadata: BTreeMap::new(),
        local_relevance: None,
        entity_grounded: None,
        freshness: None,
        engagement_score: None,
        source_quality: None,
        local_rank_score: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_atom_xml_and_drops_stale_papers() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <entry>
            <id>http://arxiv.org/abs/2610.12345v1</id>
            <published>2026-10-25T12:00:00Z</published>
            <title>Fast Rust Verification</title>
            <summary>Recent paper on verification.</summary>
            <author><name>Jane Doe</name></author>
          </entry>
          <entry>
            <id>http://arxiv.org/abs/1701.00001v1</id>
            <published>2017-01-01T00:00:00Z</published>
            <title>Ancient Paper</title>
            <summary>Too old.</summary>
          </entry>
        </feed>"#;
        let ref_date = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let items = parse_atom_entries(xml, ref_date);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_id, "2610.12345v1");
        assert_eq!(items[0].author.as_deref(), Some("Jane Doe"));
    }
}
