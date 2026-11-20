//! Core data model and versioned output contract (`radar30/research@1`).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable wire schema identifier for JSON research output.
pub const RESEARCH_SCHEMA: &str = "radar30/research@1";
/// Stable wire schema identifier for JSON preflight output.
pub const PREFLIGHT_SCHEMA: &str = "radar30/preflight@1";

/// Search depth profile controlling per-source result limits and pool size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    Quick,
    #[default]
    Default,
    Deep,
}

impl Depth {
    pub fn per_stream_limit(self) -> usize {
        match self {
            Depth::Quick => 6,
            Depth::Default => 12,
            Depth::Deep => 20,
        }
    }

    pub fn pool_limit(self) -> usize {
        match self {
            Depth::Quick => 15,
            Depth::Default => 40,
            Depth::Deep => 60,
        }
    }

    pub fn source_fetch_limit(self) -> usize {
        match self {
            Depth::Quick => 12,
            Depth::Default => 25,
            Depth::Deep => 50,
        }
    }

    pub fn engagement_keeper_slots(self) -> usize {
        match self {
            Depth::Quick => 2,
            Depth::Default => 3,
            Depth::Deep => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Depth::Quick => "quick",
            Depth::Default => "default",
            Depth::Deep => "deep",
        }
    }
}

impl FromStr for Depth {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "quick" => Ok(Depth::Quick),
            "default" => Ok(Depth::Default),
            "deep" => Ok(Depth::Deep),
            other => Err(format!(
                "invalid depth `{other}` (expected quick, default, or deep)"
            )),
        }
    }
}

/// Output format for `radar30 research`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmitFormat {
    #[default]
    Compact,
    Md,
    Json,
}

impl FromStr for EmitFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "compact" => Ok(EmitFormat::Compact),
            "md" | "markdown" => Ok(EmitFormat::Md),
            "json" => Ok(EmitFormat::Json),
            other => Err(format!(
                "invalid emit format `{other}` (expected compact, md, or json)"
            )),
        }
    }
}

/// Supported keyless sources in Wave 1.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SourceId {
    Reddit,
    HackerNews,
    GitHub,
    Polymarket,
    DevTo,
    Arxiv,
}

impl SourceId {
    pub const ALL: [SourceId; 6] = [
        SourceId::Reddit,
        SourceId::HackerNews,
        SourceId::GitHub,
        SourceId::Polymarket,
        SourceId::DevTo,
        SourceId::Arxiv,
    ];

    /// Default keyless sources enabled when `--sources` is omitted.
    /// `devto` is opt-in (matching last30days) unless requested or included.
    pub const DEFAULT_ENABLED: [SourceId; 5] = [
        SourceId::Reddit,
        SourceId::HackerNews,
        SourceId::GitHub,
        SourceId::Polymarket,
        SourceId::Arxiv,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Reddit => "reddit",
            SourceId::HackerNews => "hackernews",
            SourceId::GitHub => "github",
            SourceId::Polymarket => "polymarket",
            SourceId::DevTo => "devto",
            SourceId::Arxiv => "arxiv",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            SourceId::Reddit => "Reddit (shreddit search)",
            SourceId::HackerNews => "Hacker News (Algolia)",
            SourceId::GitHub => "GitHub (REST Search)",
            SourceId::Polymarket => "Polymarket (Gamma)",
            SourceId::DevTo => "DEV Community (Forem)",
            SourceId::Arxiv => "arXiv (Atom API)",
        }
    }

    pub fn endpoint_summary(self) -> &'static str {
        match self {
            SourceId::Reddit => "https://www.reddit.com/svc/shreddit/search/",
            SourceId::HackerNews => "https://hn.algolia.com/api/v1/search",
            SourceId::GitHub => "https://api.github.com/search/issues",
            SourceId::Polymarket => "https://gamma-api.polymarket.com/public-search",
            SourceId::DevTo => "https://dev.to/api/articles",
            SourceId::Arxiv => "https://export.arxiv.org/api/query",
        }
    }

    /// Editorial signal-to-noise quality baseline (aligned with last30days `SOURCE_QUALITY`).
    pub fn quality_weight(self) -> f64 {
        match self {
            SourceId::Arxiv => 0.90,
            SourceId::HackerNews => 0.80,
            SourceId::DevTo => 0.78,
            SourceId::GitHub => 0.78,
            SourceId::Reddit => 0.65,
            SourceId::Polymarket => 0.55,
        }
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SourceId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "reddit" | "r" => Ok(SourceId::Reddit),
            "hackernews" | "hn" | "hacker-news" => Ok(SourceId::HackerNews),
            "github" | "gh" => Ok(SourceId::GitHub),
            "polymarket" | "pm" => Ok(SourceId::Polymarket),
            "devto" | "dev.to" | "dev-to" => Ok(SourceId::DevTo),
            "arxiv" => Ok(SourceId::Arxiv),
            other => Err(format!(
                "unknown source `{other}` (valid: reddit, hackernews/hn, github/gh, polymarket/pm, devto/dev.to, arxiv)"
            )),
        }
    }
}

/// High-level query intent affecting clustering thresholds and source weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    BreakingNews,
    Opinion,
    Comparison,
    Prediction,
    General,
}

impl Intent {
    pub fn is_clusterable(self) -> bool {
        !matches!(self, Intent::General)
    }

    pub fn cluster_threshold(self) -> f64 {
        match self {
            Intent::BreakingNews => 0.42,
            _ => 0.48,
        }
    }
}

/// Date range window for a research run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateWindow {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl DateWindow {
    pub fn days(self) -> i64 {
        (self.to - self.from).num_days().max(1)
    }
}

/// Planner-emitted retrieval unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubQuery {
    pub label: String,
    pub search_query: String,
    pub ranking_query: String,
    pub sources: Vec<SourceId>,
    pub weight: f64,
}

/// Structured query plan derived from the raw user topic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryPlan {
    pub raw_topic: String,
    pub primary_entity: String,
    pub head_token: String,
    pub intent: Intent,
    pub subqueries: Vec<SubQuery>,
    pub source_weights: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Normalized engagement counters across sources.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Engagement {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stars: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
}

impl Engagement {
    /// Merge incoming counters keeping the larger positive value per field.
    pub fn merge_max(&mut self, other: &Engagement) {
        self.score = max_opt_i64(self.score, other.score);
        self.comments = max_opt_i64(self.comments, other.comments);
        self.stars = max_opt_i64(self.stars, other.stars);
        self.volume_usd = max_opt_f64(self.volume_usd, other.volume_usd);
        if self.probability.is_none() {
            self.probability = other.probability;
        }
    }

    /// Primary scalar engagement used for engagement keepers and display sorting.
    pub fn primary_metric(&self) -> f64 {
        if let Some(vol) = self.volume_usd
            && vol > 0.0
        {
            return vol;
        }
        let s = self.score.unwrap_or(0).max(0) as f64;
        let c = self.comments.unwrap_or(0).max(0) as f64;
        let st = self.stars.unwrap_or(0).max(0) as f64;
        s + c + st
    }
}

fn max_opt_i64(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn max_opt_f64(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Date confidence on an item's published date.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateConfidence {
    High,
    Med,
    #[default]
    Low,
}

/// Normalized evidence item returned by a source adapter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceItem {
    pub item_id: String,
    pub source: SourceId,
    pub title: String,
    pub body: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// ISO date (`YYYY-MM-DD`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub date_confidence: DateConfidence,
    #[serde(default)]
    pub engagement: Engagement,
    #[serde(default)]
    pub snippet: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    // Signals computed by `crate::signals`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_relevance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_grounded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engagement_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_quality: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_rank_score: Option<f64>,
}

impl SourceItem {
    pub fn published_date(&self) -> Option<NaiveDate> {
        let raw = self.published_at.as_deref()?;
        let prefix = raw.get(..10)?;
        NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok()
    }

    pub fn is_out_of_window(&self, window: DateWindow) -> bool {
        // arXiv uses a wider 365-day window (papers don't trend on a 30-day clock).
        if self.source == SourceId::Arxiv {
            return false;
        }
        let Some(date) = self.published_date() else {
            return false;
        };
        date < window.from || date > window.to
    }
}

/// Fused candidate merged across subquery streams by canonical URL / thread identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_id: String,
    pub canonical_key: String,
    pub primary_source: SourceId,
    pub sources: Vec<SourceId>,
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub out_of_window: bool,
    pub subquery_labels: Vec<String>,
    pub native_ranks: BTreeMap<String, usize>,
    pub local_relevance: f64,
    pub entity_grounded: bool,
    pub freshness: u32,
    pub engagement: Engagement,
    pub engagement_score: f64,
    pub source_quality: f64,
    pub rrf_score: f64,
    pub final_score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// Uncertainty qualifier on a cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClusterUncertainty {
    SingleSource,
    ThinEvidence,
}

/// Ranked cluster of corroborated candidates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub cluster_id: String,
    pub title: String,
    pub candidate_ids: Vec<String>,
    pub representative_ids: Vec<String>,
    pub sources: Vec<SourceId>,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<ClusterUncertainty>,
}

/// Outcome state of one source fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceState {
    Ok,
    NoResults,
    Partial,
    RateLimited,
    Forbidden,
    Timeout,
    Unreachable,
    Error,
}

/// Telemetry for one source in a research run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOutcome {
    pub source: SourceId,
    pub state: SourceState,
    pub item_count: usize,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Full versioned report produced by `pipeline::run`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchReport {
    pub schema: String,
    pub version: String,
    pub topic: String,
    pub primary_entity: String,
    pub intent: Intent,
    pub depth: Depth,
    pub window: DateWindow,
    pub generated_at: String,
    pub total_fetched: usize,
    pub candidates: Vec<Candidate>,
    pub clusters: Vec<Cluster>,
    pub source_outcomes: Vec<SourceOutcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ResearchReport {
    pub fn degraded_sources(&self) -> Vec<SourceId> {
        self.source_outcomes
            .iter()
            .filter(|o| {
                matches!(
                    o.state,
                    SourceState::RateLimited
                        | SourceState::Forbidden
                        | SourceState::Timeout
                        | SourceState::Unreachable
                        | SourceState::Error
                )
            })
            .map(|o| o.source)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_parses_aliases() {
        assert_eq!("hn".parse::<SourceId>().unwrap(), SourceId::HackerNews);
        assert_eq!("dev.to".parse::<SourceId>().unwrap(), SourceId::DevTo);
        assert_eq!("pm".parse::<SourceId>().unwrap(), SourceId::Polymarket);
        assert_eq!("gh".parse::<SourceId>().unwrap(), SourceId::GitHub);
        assert!("unknown".parse::<SourceId>().is_err());
    }

    #[test]
    fn engagement_merge_keeps_largest_counters() {
        let mut a = Engagement {
            score: Some(10),
            comments: Some(50),
            stars: None,
            volume_usd: None,
            probability: None,
        };
        let b = Engagement {
            score: Some(120),
            comments: Some(20),
            stars: Some(500),
            volume_usd: None,
            probability: Some(0.75),
        };
        a.merge_max(&b);
        assert_eq!(a.score, Some(120));
        assert_eq!(a.comments, Some(50));
        assert_eq!(a.stars, Some(500));
        assert_eq!(a.probability, Some(0.75));
    }
}
