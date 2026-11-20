//! Persistent SQLite store (`~/.local/share/radar30/research.db`) with WAL mode,
//! FTS5 full-text search, sighting accumulation, watchlist topics, and discovery topic queue.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::schema::ResearchReport;
use crate::setup;

const SCHEMA_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS topics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    schedule TEXT NOT NULL DEFAULT '0 8 * * *',
    sources TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS research_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    topic_id INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    run_date TEXT NOT NULL,
    depth TEXT NOT NULL,
    window_from TEXT NOT NULL,
    window_to TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'completed',
    findings_new INTEGER NOT NULL DEFAULT 0,
    findings_updated INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS findings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id INTEGER REFERENCES research_runs(id) ON DELETE SET NULL,
    topic_id INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    source_url TEXT NOT NULL UNIQUE,
    source_title TEXT NOT NULL,
    snippet TEXT NOT NULL DEFAULT '',
    published_at TEXT,
    final_score REAL NOT NULL DEFAULT 0.0,
    engagement_score REAL NOT NULL DEFAULT 0.0,
    first_seen TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen TEXT NOT NULL DEFAULT (datetime('now')),
    sighting_count INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_findings_topic_seen ON findings(topic_id, last_seen DESC);
CREATE INDEX IF NOT EXISTS idx_findings_first_seen ON findings(first_seen DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS findings_fts USING fts5(
    source_title,
    snippet,
    source,
    content='findings',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS findings_ai AFTER INSERT ON findings BEGIN
    INSERT INTO findings_fts(rowid, source_title, snippet, source)
    VALUES (new.id, new.source_title, new.snippet, new.source);
END;

CREATE TRIGGER IF NOT EXISTS findings_ad AFTER DELETE ON findings BEGIN
    INSERT INTO findings_fts(findings_fts, rowid, source_title, snippet, source)
    VALUES ('delete', old.id, old.source_title, old.snippet, old.source);
END;

CREATE TRIGGER IF NOT EXISTS findings_au AFTER UPDATE OF source_title, snippet, source ON findings BEGIN
    INSERT INTO findings_fts(findings_fts, rowid, source_title, snippet, source)
    VALUES ('delete', old.id, old.source_title, old.snippet, old.source);
    INSERT INTO findings_fts(rowid, source_title, snippet, source)
    VALUES (new.id, new.source_title, new.snippet, new.source);
END;

CREATE TABLE IF NOT EXISTS topic_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    normalized_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'surfaced',
    surfaced_count INTEGER NOT NULL DEFAULT 1,
    last_velocity REAL NOT NULL DEFAULT 0.0,
    first_surfaced_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_surfaced_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_version(version) VALUES (1);
"#;

/// Default database path (`$RADAR30_DB_PATH` or `~/.local/share/radar30/research.db`).
pub fn default_db_path() -> Result<PathBuf> {
    if let Ok(over) = std::env::var("RADAR30_DB_PATH")
        && !over.trim().is_empty()
    {
        return Ok(PathBuf::from(over));
    }
    Ok(setup::home()?
        .join(".local")
        .join("share")
        .join("radar30")
        .join("research.db"))
}

/// Default saved-briefs directory (`$RADAR30_BRIEFS_DIR` or `~/Documents/Radar30`).
pub fn default_briefs_dir() -> Result<PathBuf> {
    if let Ok(over) = std::env::var("RADAR30_BRIEFS_DIR")
        && !over.trim().is_empty()
    {
        return Ok(PathBuf::from(over));
    }
    Ok(setup::home()?.join("Documents").join("Radar30"))
}

/// Summary returned after persisting a `ResearchReport`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistSummary {
    pub db_path: PathBuf,
    pub run_id: i64,
    pub topic_id: i64,
    pub findings_new: usize,
    pub findings_updated: usize,
}

/// Watchlist topic row with latest run telemetry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicRecord {
    pub id: i64,
    pub name: String,
    pub schedule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    pub total_findings: i64,
}

/// Stored finding row returned by FTS5 search or briefing queries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredFinding {
    pub id: i64,
    pub topic: String,
    pub source: String,
    pub url: String,
    pub title: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub final_score: f64,
    pub engagement_score: f64,
    pub first_seen: String,
    pub last_seen: String,
    pub sighting_count: i64,
}

/// Topic queue row tracking discovery surfacing & `covered` state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueEntry {
    pub name: String,
    pub status: String,
    pub surfaced_count: i64,
    pub last_velocity: f64,
    pub first_surfaced_at: String,
    pub last_surfaced_at: String,
}

/// Handle to the SQLite research store.
pub struct Store {
    path: PathBuf,
    conn: Connection,
}

impl Store {
    /// Open (or create) the SQLite database at `path` and ensure schema + WAL are ready.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("opening SQLite store {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA_SQL)
            .with_context(|| format!("initializing schema in {}", path.display()))?;
        harden_permissions(&path);
        Ok(Self { path, conn })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ensure a topic exists and return its ID.
    pub fn ensure_topic(
        &self,
        name: &str,
        schedule: Option<&str>,
        sources: Option<&str>,
    ) -> Result<i64> {
        let clean = name.trim();
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM topics WHERE name = ?1 COLLATE NOCASE",
                params![clean],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            if schedule.is_some() || sources.is_some() {
                self.conn.execute(
                    "UPDATE topics
                     SET schedule = COALESCE(?2, schedule),
                         sources = COALESCE(?3, sources),
                         enabled = 1,
                         updated_at = datetime('now')
                     WHERE id = ?1",
                    params![id, schedule, sources],
                )?;
            }
            return Ok(id);
        }
        let sched = schedule.unwrap_or("0 8 * * *");
        self.conn.execute(
            "INSERT INTO topics (name, schedule, sources, enabled) VALUES (?1, ?2, ?3, 1)",
            params![clean, sched, sources],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Remove a watchlist topic (cascades its runs and findings). Returns `true` if deleted.
    pub fn remove_topic(&self, name: &str) -> Result<bool> {
        let rows = self.conn.execute(
            "DELETE FROM topics WHERE name = ?1 COLLATE NOCASE",
            params![name.trim()],
        )?;
        Ok(rows > 0)
    }

    /// Pause or resume a topic.
    pub fn set_topic_enabled(&self, name: &str, enabled: bool) -> Result<bool> {
        let rows = self.conn.execute(
            "UPDATE topics SET enabled = ?2, updated_at = datetime('now') WHERE name = ?1 COLLATE NOCASE",
            params![name.trim(), i32::from(enabled)],
        )?;
        Ok(rows > 0)
    }

    /// List all topics with latest run status and accumulated finding count.
    pub fn list_topics(&self) -> Result<Vec<TopicRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, t.schedule, t.sources, t.enabled, t.created_at,
                    (SELECT r.created_at FROM research_runs r WHERE r.topic_id = t.id ORDER BY r.id DESC LIMIT 1),
                    (SELECT r.status FROM research_runs r WHERE r.topic_id = t.id ORDER BY r.id DESC LIMIT 1),
                    (SELECT COUNT(*) FROM findings f WHERE f.topic_id = t.id)
             FROM topics t
             ORDER BY t.name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TopicRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                schedule: row.get(2)?,
                sources: row.get(3)?,
                enabled: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
                last_run_at: row.get(6)?,
                last_status: row.get(7)?,
                total_findings: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Persist a completed `ResearchReport`: creates a `research_runs` row and upserts
    /// each candidate into `findings` (deduping by `source_url`, incrementing `sighting_count`).
    pub fn record_report(&mut self, report: &ResearchReport) -> Result<PersistSummary> {
        let topic_id = self.ensure_topic(&report.topic, None, None)?;
        let tx = self.conn.transaction()?;
        let run_date = Utc::now().format("%Y-%m-%d").to_string();
        let status = if report.degraded_sources().is_empty() {
            "completed"
        } else {
            "degraded"
        };

        tx.execute(
            "INSERT INTO research_runs (topic_id, run_date, depth, window_from, window_to, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                topic_id,
                run_date,
                report.depth.as_str(),
                report.window.from.to_string(),
                report.window.to.to_string(),
                status,
            ],
        )?;
        let run_id = tx.last_insert_rowid();

        let mut findings_new = 0usize;
        let mut findings_updated = 0usize;

        for cand in &report.candidates {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM findings WHERE source_url = ?1",
                    params![cand.url],
                    |row| row.get(0),
                )
                .optional()?;

            if let Some(fid) = existing {
                tx.execute(
                    "UPDATE findings
                     SET run_id = ?2,
                         final_score = MAX(final_score, ?3),
                         engagement_score = MAX(engagement_score, ?4),
                         snippet = CASE WHEN length(?5) > length(snippet) THEN ?5 ELSE snippet END,
                         last_seen = datetime('now'),
                         sighting_count = sighting_count + 1
                     WHERE id = ?1",
                    params![
                        fid,
                        run_id,
                        cand.final_score,
                        cand.engagement_score,
                        cand.snippet
                    ],
                )?;
                findings_updated += 1;
            } else {
                tx.execute(
                    "INSERT INTO findings
                     (run_id, topic_id, source, source_url, source_title, snippet, published_at, final_score, engagement_score)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        run_id,
                        topic_id,
                        cand.primary_source.as_str(),
                        cand.url,
                        cand.title,
                        cand.snippet,
                        cand.published_at,
                        cand.final_score,
                        cand.engagement_score,
                    ],
                )?;
                findings_new += 1;
            }
        }

        tx.execute(
            "UPDATE research_runs SET findings_new = ?2, findings_updated = ?3 WHERE id = ?1",
            params![run_id, findings_new as i64, findings_updated as i64],
        )?;
        tx.commit()?;

        Ok(PersistSummary {
            db_path: self.path.clone(),
            run_id,
            topic_id,
            findings_new,
            findings_updated,
        })
    }

    /// Full-text search stored findings via FTS5 (`porter unicode61`), falling back to `LIKE`
    /// if the query uses syntax characters that FTS5 rejects.
    pub fn search_findings(&self, query: &str, limit: usize) -> Result<Vec<StoredFinding>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let sql = "SELECT f.id, t.name, f.source, f.source_url, f.source_title, f.snippet,
                          f.published_at, f.final_score, f.engagement_score,
                          f.first_seen, f.last_seen, f.sighting_count
                   FROM findings_fts
                   JOIN findings f ON findings_fts.rowid = f.id
                   JOIN topics t ON t.id = f.topic_id
                   WHERE findings_fts MATCH ?1
                   ORDER BY bm25(findings_fts), f.final_score DESC
                   LIMIT ?2";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![fts_query, limit as i64], map_finding)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Return findings first seen on or after `since_iso` (`YYYY-MM-DD`) for one topic.
    pub fn findings_since(&self, topic_id: i64, since_iso: &str) -> Result<Vec<StoredFinding>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, t.name, f.source, f.source_url, f.source_title, f.snippet,
                    f.published_at, f.final_score, f.engagement_score,
                    f.first_seen, f.last_seen, f.sighting_count
             FROM findings f
             JOIN topics t ON t.id = f.topic_id
             WHERE f.topic_id = ?1 AND f.first_seen >= ?2
             ORDER BY f.final_score DESC, f.engagement_score DESC",
        )?;
        let rows = stmt.query_map(params![topic_id, since_iso], map_finding)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Record a surfaced discovery topic in `topic_queue`, returning its prior sighting state
    /// (for annotate-only repeat badges like "surfaced 2nd time" or preserving `covered`).
    pub fn record_queue_sighting(
        &self,
        name: &str,
        velocity: f64,
    ) -> Result<(Option<QueueEntry>, QueueEntry)> {
        let clean = name.trim();
        let norm = normalize_queue_name(clean);

        let prior: Option<QueueEntry> = self
            .conn
            .query_row(
                "SELECT name, status, surfaced_count, last_velocity, first_surfaced_at, last_surfaced_at
                 FROM topic_queue
                 WHERE normalized_name = ?1
                 ORDER BY id ASC
                 LIMIT 1",
                params![norm],
                |row| {
                    Ok(QueueEntry {
                        name: row.get(0)?,
                        status: row.get(1)?,
                        surfaced_count: row.get(2)?,
                        last_velocity: row.get(3)?,
                        first_surfaced_at: row.get(4)?,
                        last_surfaced_at: row.get(5)?,
                    })
                },
            )
            .optional()?;

        if let Some(ref prev) = prior {
            self.conn.execute(
                "UPDATE topic_queue
                 SET surfaced_count = surfaced_count + 1,
                     last_velocity = ?2,
                     last_surfaced_at = datetime('now')
                 WHERE normalized_name = ?1",
                params![norm, velocity],
            )?;
            let updated = QueueEntry {
                name: prev.name.clone(),
                status: prev.status.clone(),
                surfaced_count: prev.surfaced_count + 1,
                last_velocity: velocity,
                first_surfaced_at: prev.first_surfaced_at.clone(),
                last_surfaced_at: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            };
            Ok((prior, updated))
        } else {
            self.conn.execute(
                "INSERT INTO topic_queue (name, normalized_name, status, surfaced_count, last_velocity)
                 VALUES (?1, ?2, 'surfaced', 1, ?3)",
                params![clean, norm, velocity],
            )?;
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let created = QueueEntry {
                name: clean.to_string(),
                status: "surfaced".to_string(),
                surfaced_count: 1,
                last_velocity: velocity,
                first_surfaced_at: now.clone(),
                last_surfaced_at: now,
            };
            Ok((None, created))
        }
    }

    /// Mark a discovery topic queue row as `covered`.
    pub fn mark_topic_covered(&self, name: &str) -> Result<bool> {
        let norm = normalize_queue_name(name);
        let rows = self.conn.execute(
            "UPDATE topic_queue SET status = 'covered' WHERE normalized_name = ?1 OR name = ?2 COLLATE NOCASE",
            params![norm, name.trim()],
        )?;
        Ok(rows > 0)
    }

    /// List all discovery topic queue rows, newest first.
    pub fn list_queue(&self) -> Result<Vec<QueueEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, status, surfaced_count, last_velocity, first_surfaced_at, last_surfaced_at
             FROM topic_queue
             ORDER BY last_surfaced_at DESC, last_velocity DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(QueueEntry {
                name: row.get(0)?,
                status: row.get(1)?,
                surfaced_count: row.get(2)?,
                last_velocity: row.get(3)?,
                first_surfaced_at: row.get(4)?,
                last_surfaced_at: row.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Set a key/value setting (e.g. `delivery_channel`).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
}

fn map_finding(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredFinding> {
    Ok(StoredFinding {
        id: row.get(0)?,
        topic: row.get(1)?,
        source: row.get(2)?,
        url: row.get(3)?,
        title: row.get(4)?,
        snippet: row.get(5)?,
        published_at: row.get(6)?,
        final_score: row.get(7)?,
        engagement_score: row.get(8)?,
        first_seen: row.get(9)?,
        last_seen: row.get(10)?,
        sighting_count: row.get(11)?,
    })
}

fn sanitize_fts_query(raw: &str) -> String {
    let tokens: Vec<String> = crate::query::tokenize(raw)
        .into_iter()
        .filter(|t| t.len() >= 2)
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    tokens.join(" OR ")
}

fn normalize_queue_name(name: &str) -> String {
    crate::query::tokenize(name).join(" ")
}

#[cfg(unix)]
fn harden_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn harden_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Candidate, DateWindow, Depth, Engagement, Intent, RESEARCH_SCHEMA, SourceId,
    };
    use chrono::NaiveDate;
    use std::collections::BTreeMap;

    fn sample_report(topic: &str, url: &str, score: f64) -> ResearchReport {
        ResearchReport {
            schema: RESEARCH_SCHEMA.to_string(),
            version: "0.2.0".to_string(),
            topic: topic.to_string(),
            primary_entity: topic.to_string(),
            intent: Intent::BreakingNews,
            depth: Depth::Quick,
            window: DateWindow {
                from: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
                to: NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
            },
            generated_at: "2026-11-19T12:00:00Z".to_string(),
            total_fetched: 1,
            candidates: vec![Candidate {
                candidate_id: "cand-1".to_string(),
                canonical_key: url.to_string(),
                primary_source: SourceId::HackerNews,
                sources: vec![SourceId::HackerNews],
                title: "Rust 1.100 incremental linker breakthrough".to_string(),
                url: url.to_string(),
                snippet: "Linker speedups across large Rust workspaces.".to_string(),
                published_at: Some("2026-11-18".to_string()),
                out_of_window: false,
                subquery_labels: vec!["primary".to_string()],
                native_ranks: BTreeMap::new(),
                local_relevance: 0.95,
                entity_grounded: true,
                freshness: 98,
                engagement: Engagement {
                    score: Some(500),
                    comments: Some(120),
                    ..Default::default()
                },
                engagement_score: 0.88,
                source_quality: 0.80,
                rrf_score: 0.015,
                final_score: score,
                cluster_id: Some("cluster-1".to_string()),
                metadata: BTreeMap::new(),
            }],
            clusters: vec![],
            source_outcomes: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn store_persists_dedupes_searches_fts5_and_tracks_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("research.db");
        let mut store = Store::open(&db).unwrap();

        let r1 = sample_report("Rust", "https://blog.rust-lang.org/1.100", 85.0);
        let p1 = store.record_report(&r1).unwrap();
        assert_eq!(p1.findings_new, 1);
        assert_eq!(p1.findings_updated, 0);

        // Re-recording the same URL updates sighting_count instead of duplicating.
        let r2 = sample_report("Rust", "https://blog.rust-lang.org/1.100", 92.0);
        let p2 = store.record_report(&r2).unwrap();
        assert_eq!(p2.findings_new, 0);
        assert_eq!(p2.findings_updated, 1);

        let hits = store.search_findings("incremental linker", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].sighting_count, 2);
        assert_eq!(hits[0].final_score, 92.0);

        // Queue tracking + covered persistence.
        let (prior1, cur1) = store.record_queue_sighting("Rust 1.100", 88.5).unwrap();
        assert!(prior1.is_none());
        assert_eq!(cur1.surfaced_count, 1);

        assert!(store.mark_topic_covered("rust 1.100").unwrap());
        let (prior2, cur2) = store.record_queue_sighting("Rust 1.100", 91.0).unwrap();
        assert_eq!(prior2.unwrap().status, "covered");
        assert_eq!(cur2.status, "covered");
        assert_eq!(cur2.surfaced_count, 2);
    }
}
