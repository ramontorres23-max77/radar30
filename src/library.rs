//! Offline deterministic research library: saving briefs, searching saved Markdown + SQLite
//! sightings, and generating a subscribable static `index.html` + Atom `feed.xml`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use crate::query::tokenize;
use crate::render;
use crate::schema::{EmitFormat, ResearchReport};
use crate::store::{Store, StoredFinding};

/// One saved Markdown brief discovered in the library directory.
#[derive(Clone, Debug, Serialize)]
pub struct BriefMeta {
    pub slug: String,
    pub title: String,
    pub date: String,
    pub path: PathBuf,
    pub summary: String,
}

/// Unified search hit from either saved Markdown briefs or the SQLite store.
#[derive(Clone, Debug, Serialize)]
pub struct LibraryHit {
    pub kind: &'static str,
    pub title: String,
    pub date: String,
    pub location: String,
    pub excerpt: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LibrarySearchResult {
    pub query: String,
    pub hits: Vec<LibraryHit>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FeedBuildResult {
    pub briefs_count: usize,
    pub out_dir: PathBuf,
    pub index_html: PathBuf,
    pub feed_xml: PathBuf,
}

/// Save a Markdown brief into `briefs_dir/<slug>-<YYYY-MM-DD>.md` and return its path.
pub fn save_brief(report: &ResearchReport, briefs_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(briefs_dir)
        .with_context(|| format!("creating {}", briefs_dir.display()))?;
    let slug = slugify(&report.topic);
    let date = report.window.to.format("%Y-%m-%d");
    let path = briefs_dir.join(format!("{slug}-{date}.md"));
    let md = render::render(report, EmitFormat::Md)?;
    std::fs::write(&path, md).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Scan `briefs_dir` for saved `.md` briefs, sorted newest first.
pub fn scan_briefs(briefs_dir: &Path) -> Result<Vec<BriefMeta>> {
    if !briefs_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut briefs = Vec::new();
    for entry in std::fs::read_dir(briefs_dir)
        .with_context(|| format!("reading {}", briefs_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("brief")
            .to_string();
        let title = extract_title(&content).unwrap_or_else(|| stem.clone());
        let date = extract_date(&stem, &content);
        let summary = extract_summary(&content);
        briefs.push(BriefMeta {
            slug: stem,
            title,
            date,
            path,
            summary,
        });
    }
    briefs.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.title.cmp(&b.title)));
    Ok(briefs)
}

/// Search both saved Markdown briefs and SQLite findings deterministically offline.
pub fn search(
    query: &str,
    briefs_dir: &Path,
    db_path: Option<&Path>,
    limit: usize,
) -> Result<LibrarySearchResult> {
    let q_tokens: Vec<String> = tokenize(query)
        .into_iter()
        .filter(|t| t.len() >= 2)
        .collect();
    let mut hits = Vec::new();

    if !q_tokens.is_empty() {
        for brief in scan_briefs(briefs_dir)? {
            let Ok(text) = std::fs::read_to_string(&brief.path) else {
                continue;
            };
            let lower = text.to_ascii_lowercase();
            let matched = q_tokens
                .iter()
                .filter(|t| lower.contains(t.as_str()))
                .count();
            if matched == 0 {
                continue;
            }
            let ratio = matched as f64 / q_tokens.len() as f64;
            let title_bonus = if brief.title.to_ascii_lowercase().contains(&q_tokens[0]) {
                0.25
            } else {
                0.0
            };
            hits.push(LibraryHit {
                kind: "brief",
                title: brief.title,
                date: brief.date,
                location: brief.path.display().to_string(),
                excerpt: brief.summary,
                score: ((ratio + title_bonus) * 100.0).round() / 100.0,
            });
        }
    }

    if let Some(db) = db_path
        && db.is_file()
        && let Ok(store) = Store::open(db)
        && let Ok(stored) = store.search_findings(query, limit)
    {
        for f in stored {
            hits.push(finding_to_hit(f));
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| a.title.cmp(&b.title))
    });
    hits.truncate(limit);

    Ok(LibrarySearchResult {
        query: query.to_string(),
        hits,
    })
}

fn finding_to_hit(f: StoredFinding) -> LibraryHit {
    let date = f
        .published_at
        .unwrap_or_else(|| f.first_seen.chars().take(10).collect());
    LibraryHit {
        kind: "finding",
        title: format!("[{}] {}", f.source, f.title),
        date,
        location: f.url,
        excerpt: f.snippet,
        score: (f.final_score / 100.0).clamp(0.0, 1.5),
    }
}

/// Build static HTML pages, `index.html`, and Atom `feed.xml` from saved briefs.
pub fn build_feed(briefs_dir: &Path, out_dir: &Path, site_url: &str) -> Result<FeedBuildResult> {
    let briefs = scan_briefs(briefs_dir)?;
    let pages_dir = out_dir.join("briefs");
    std::fs::create_dir_all(&pages_dir)
        .with_context(|| format!("creating {}", pages_dir.display()))?;

    let base = site_url.trim_end_matches('/');
    let mut index_rows = String::new();
    let mut atom_entries = String::new();

    for b in &briefs {
        let md = std::fs::read_to_string(&b.path).unwrap_or_default();
        let page_name = format!("{}.html", b.slug);
        let page_path = pages_dir.join(&page_name);
        std::fs::write(&page_path, render_brief_html(&b.title, &b.date, &md))?;

        let _ = writeln!(
            index_rows,
            "<li><span class=\"date\">{}</span> <a href=\"briefs/{}\">{}</a><p>{}</p></li>",
            escape_html(&b.date),
            escape_html(&page_name),
            escape_html(&b.title),
            escape_html(&b.summary)
        );

        let entry_link = format!("{base}/briefs/{page_name}");
        let _ = writeln!(
            atom_entries,
            "  <entry>\n    <id>{}</id>\n    <title>{}</title>\n    <updated>{}T00:00:00Z</updated>\n    <link href=\"{}\"/>\n    <summary>{}</summary>\n  </entry>",
            escape_xml(&entry_link),
            escape_xml(&b.title),
            escape_xml(&b.date),
            escape_xml(&entry_link),
            escape_xml(&b.summary),
        );
    }

    let index_html = out_dir.join("index.html");
    let index_doc = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>radar30 Research Library</title>\n<link rel=\"alternate\" type=\"application/atom+xml\" title=\"radar30 Feed\" href=\"feed.xml\">\n<style>body{{font-family:system-ui,-apple-system,sans-serif;max-width:820px;margin:2rem auto;padding:0 1rem;line-height:1.55;color:#111}}ul{{list-style:none;padding:0}}li{{margin:1rem 0;padding-bottom:1rem;border-bottom:1px solid #e5e7eb}}.date{{color:#6b7280;font-size:.9rem;margin-right:.5rem}}a{{color:#0f52ba;font-weight:600;text-decoration:none}}a:hover{{text-decoration:underline}}p{{margin:.35rem 0 0;color:#374151}}</style>\n</head>\n<body>\n<h1>radar30 Research Library</h1>\n<p><a href=\"feed.xml\">Atom feed</a> · {} saved brief(s)</p>\n<ul>\n{index_rows}</ul>\n</body>\n</html>\n",
        briefs.len()
    );
    std::fs::write(&index_html, index_doc)?;

    let updated = Utc::now().to_rfc3339();
    let feed_xml = out_dir.join("feed.xml");
    let feed_doc = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n  <id>{}/feed.xml</id>\n  <title>radar30 Research Library</title>\n  <updated>{}</updated>\n  <link rel=\"self\" href=\"{}/feed.xml\"/>\n  <link href=\"{}/\"/>\n{atom_entries}</feed>\n",
        escape_xml(base),
        escape_xml(&updated),
        escape_xml(base),
        escape_xml(base),
    );
    std::fs::write(&feed_xml, feed_doc)?;

    Ok(FeedBuildResult {
        briefs_count: briefs.len(),
        out_dir: out_dir.to_path_buf(),
        index_html,
        feed_xml,
    })
}

/// Render a saved Markdown brief as a standalone local HTML page.
pub fn publish_markdown(input: &Path, out_dir: &Path) -> Result<PathBuf> {
    let md =
        std::fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("brief")
        .to_string();
    let title = extract_title(&md).unwrap_or_else(|| stem.clone());
    let date = extract_date(&stem, &md);
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let target = out_dir.join(format!("{stem}.html"));
    std::fs::write(&target, render_brief_html(&title, &date, &md))
        .with_context(|| format!("writing {}", target.display()))?;
    Ok(target)
}

fn render_brief_html(title: &str, date: &str, md: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{}</title>\n<style>body{{font-family:system-ui,-apple-system,sans-serif;max-width:860px;margin:2rem auto;padding:0 1rem;line-height:1.6}}pre{{white-space:pre-wrap;font-family:inherit;background:#f9fafb;padding:1rem;border-radius:8px;border:1px solid #e5e7eb}}</style>\n</head>\n<body>\n<p><a href=\"../index.html\">← Library</a> · {}</p>\n<h1>{}</h1>\n<pre>{}</pre>\n</body>\n</html>\n",
        escape_html(title),
        escape_html(date),
        escape_html(title),
        escape_html(md),
    )
}

pub fn format_search_text(res: &LibrarySearchResult) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "radar30 library search: \"{}\"", res.query);
    if res.hits.is_empty() {
        let _ = writeln!(out, "  No matching briefs or stored findings.");
        return out;
    }
    for (i, hit) in res.hits.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}. [{}] {} ({}) — {}",
            i + 1,
            hit.kind,
            hit.title,
            hit.date,
            hit.location
        );
        if !hit.excerpt.is_empty() {
            let _ = writeln!(out, "   {}", hit.excerpt);
        }
    }
    out
}

pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "brief".to_string()
    } else {
        trimmed.chars().take(64).collect()
    }
}

fn extract_title(md: &str) -> Option<String> {
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            let cleaned = rest
                .trim()
                .strip_prefix("Research Brief:")
                .unwrap_or(rest.trim())
                .trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

fn extract_date(stem: &str, md: &str) -> String {
    if stem.len() >= 10 {
        let tail = &stem[stem.len() - 10..];
        if chrono::NaiveDate::parse_from_str(tail, "%Y-%m-%d").is_ok() {
            return tail.to_string();
        }
    }
    for line in md.lines().take(12) {
        if let Some(idx) = line.find(" to ")
            && let Some(candidate) = line.get(idx + 4..idx + 14)
            && chrono::NaiveDate::parse_from_str(candidate, "%Y-%m-%d").is_ok()
        {
            return candidate.to_string();
        }
    }
    Utc::now().format("%Y-%m-%d").to_string()
}

fn extract_summary(md: &str) -> String {
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("### 1. ") {
            return trimmed.trim_start_matches("### 1. ").to_string();
        }
    }
    md.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
        .unwrap_or("")
        .chars()
        .take(180)
        .collect()
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml(s: &str) -> String {
    escape_html(s).replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DateWindow, Depth, Intent, RESEARCH_SCHEMA};
    use chrono::NaiveDate;

    #[test]
    fn library_saves_searches_and_builds_atom_feed() {
        let tmp = tempfile::tempdir().unwrap();
        let briefs_dir = tmp.path().join("briefs");
        let site_dir = tmp.path().join("site");

        let report = ResearchReport {
            schema: RESEARCH_SCHEMA.to_string(),
            version: "0.2.0".to_string(),
            topic: "MCP Servers".to_string(),
            primary_entity: "MCP Servers".to_string(),
            intent: Intent::BreakingNews,
            depth: Depth::Quick,
            window: DateWindow {
                from: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
                to: NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
            },
            generated_at: "2026-11-19T12:00:00Z".to_string(),
            total_fetched: 0,
            candidates: vec![],
            clusters: vec![],
            source_outcomes: vec![],
            warnings: vec![],
        };

        let saved = save_brief(&report, &briefs_dir).unwrap();
        assert!(saved.is_file());

        let hits = search("MCP servers", &briefs_dir, None, 10).unwrap();
        assert_eq!(hits.hits.len(), 1);
        assert_eq!(hits.hits[0].kind, "brief");

        let built = build_feed(&briefs_dir, &site_dir, "https://example.org/radar30").unwrap();
        assert_eq!(built.briefs_count, 1);
        assert!(built.index_html.is_file());
        let atom = std::fs::read_to_string(&built.feed_xml).unwrap();
        assert!(atom.contains("<feed xmlns=\"http://www.w3.org/2005/Atom\">"));
        assert!(atom.contains("MCP Servers"));
    }
}
