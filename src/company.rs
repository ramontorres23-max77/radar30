//! Company intelligence: keyless US patent publications, careers-page job signals,
//! and a 30-day stock-vs-benchmark line. Designed for local runs with no API keys.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::http::{self, HttpError};
use crate::schema::DateWindow;

pub const COMPANY_SCHEMA: &str = "radar30/company@1";

const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const PATENTS_URL: &str = "https://patents.google.com/xhr/query";
const NASDAQ_CHART_URL: &str = "https://api.nasdaq.com/api/quote";
const BENCHMARK_TICKER: &str = "VOO"; // S&P 500 ETF proxy (keyless Nasdaq endpoint)

/// Ticker hints for common company names (override with `--ticker`).
const TICKER_HINTS: &[(&str, &str)] = &[
    ("apple", "AAPL"),
    ("alphabet", "GOOGL"),
    ("google", "GOOGL"),
    ("microsoft", "MSFT"),
    ("nvidia", "NVDA"),
    ("meta", "META"),
    ("amazon", "AMZN"),
    ("netflix", "NFLX"),
    ("tesla", "TSLA"),
];

/// Curated job-board overrides `company (lowercase) -> board spec`.
const BOARD_HINTS: &[(&str, &str)] = &[
    ("stripe", "greenhouse:stripe"),
    ("linear", "ashby:linear"),
    ("ramp", "ashby:ramp"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatentItem {
    pub number: String,
    pub title: String,
    pub assignee: String,
    pub priority_date: Option<String>,
    pub url: String,
    pub is_candidate_signal: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobItem {
    pub title: String,
    pub url: String,
    pub location: String,
    pub team: String,
    pub published_at: Option<String>,
    pub board: String,
    pub is_secret_signal: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinanceLine {
    pub ticker: String,
    pub benchmark_ticker: String,
    pub company_change_pct: f64,
    pub benchmark_change_pct: f64,
    pub last_price: Option<String>,
    pub compared_days: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompanyReport {
    pub schema: String,
    pub version: String,
    pub company: String,
    pub generated_at: String,
    pub window: DateWindow,
    pub patents: Vec<PatentItem>,
    pub jobs: Vec<JobItem>,
    pub finance: Option<FinanceLine>,
    pub warnings: Vec<String>,
}

/// Options for one company-intelligence run.
#[derive(Clone, Debug)]
pub struct CompanyOptions {
    pub company: String,
    pub window: DateWindow,
    pub ticker: Option<String>,
    /// Comma-separated `company=greenhouse:token` overrides (RADAR30_JOB_BOARDS).
    pub boards_override: Option<String>,
    pub timeout: Duration,
}

pub fn run(opts: &CompanyOptions) -> Result<CompanyReport> {
    let company = opts.company.trim();
    if company.is_empty() {
        anyhow::bail!("company cannot be empty");
    }
    let deadline = Instant::now() + opts.timeout;
    let timeout = opts.timeout.min(Duration::from_secs(20));
    let mut warnings = Vec::new();

    // 1. Patent Digest — Google Patents XHR, filtered to US publications in window.
    let patents = match fetch_patents(company, opts.window, timeout, deadline) {
        Ok(p) => p,
        Err(e) => {
            warnings.push(format!("patents: {e}"));
            Vec::new()
        }
    };

    // 2. Jobs Digest — careers boards (Greenhouse, Ashby, Lever, Workday).
    let boards = resolve_boards(company, opts.boards_override.as_deref());
    let mut jobs = Vec::new();
    for board in boards {
        match fetch_jobs(&board, timeout, deadline) {
            Ok(mut found) => jobs.append(&mut found),
            Err(e) => warnings.push(format!("jobs {board}: {e}")),
        }
    }
    jobs.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    jobs.dedup_by(|a, b| a.url == b.url);
    jobs.truncate(40);

    // 3. Finance line — Nasdaq keyless chart API (VOO as S&P 500 proxy).
    let ticker = opts
        .ticker
        .clone()
        .or_else(|| hint(TICKER_HINTS, company).map(ToOwned::to_owned));
    let finance = match ticker {
        Some(t) => match fetch_finance(&t, opts.window, timeout, deadline) {
            Ok(f) => Some(f),
            Err(e) => {
                warnings.push(format!("finance {t}: {e}"));
                None
            }
        },
        None => None,
    };

    Ok(CompanyReport {
        schema: COMPANY_SCHEMA.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        company: company.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        window: opts.window,
        patents,
        jobs,
        finance,
        warnings,
    })
}

fn hint(table: &[(&'static str, &'static str)], company: &str) -> Option<&'static str> {
    let lower = company.to_ascii_lowercase();
    table
        .iter()
        .find(|(needle, _)| lower.contains(*needle))
        .map(|(_, value)| *value)
}

// ---------------------------------------------------------------------------
// Patents (Google Patents public XHR endpoint)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PatentsEnvelope {
    results: PatentResults,
}

#[derive(Deserialize)]
struct PatentResults {
    #[serde(default)]
    cluster: Vec<PatentCluster>,
}

#[derive(Deserialize)]
struct PatentCluster {
    #[serde(default)]
    result: Vec<PatentEntry>,
}

#[derive(Deserialize)]
struct PatentEntry {
    patent: PatentDoc,
}

#[derive(Deserialize)]
struct PatentDoc {
    #[serde(default)]
    title: String,
    #[serde(rename = "priority_date", default)]
    priority_date: Option<String>,
    #[serde(rename = "publication_number", default)]
    publication_number: Option<String>,
    assignee: Option<Value>,
}

fn fetch_patents(
    company: &str,
    window: DateWindow,
    timeout: Duration,
    deadline: Instant,
) -> Result<Vec<PatentItem>, HttpError> {
    let after = window.from.format("%Y%m%d").to_string();
    let q = format!("assignee=({company})&after=priority:{after}");
    let mut url = Url::parse(PATENTS_URL).map_err(|e| HttpError::Decode {
        detail: e.to_string(),
    })?;
    url.query_pairs_mut()
        .append_pair("url", &q)
        .append_pair("exp", "");
    let parsed: PatentsEnvelope = http::get_json(url.as_str(), &[], timeout, deadline, 1)?;

    let mut out = Vec::new();
    for cluster in parsed.results.cluster {
        for entry in cluster.result {
            let p = entry.patent;
            let number = p.publication_number.unwrap_or_default();
            // Only US publications in the window (the digest is explicitly US-focused).
            if !number.starts_with("US") {
                continue;
            }
            if let Some(ref d) = p.priority_date
                && let Ok(date) = NaiveDate::parse_from_str(d, "%Y-%m-%d")
                && (date < window.from || date > window.to)
            {
                continue;
            }
            out.push(PatentItem {
                url: format!("https://patents.google.com/patent/{number}/en"),
                is_candidate_signal: is_candidate_patent(&p.title),
                title: decode_html_entities(&p.title),
                assignee: value_to_text(p.assignee.as_ref()),
                priority_date: p.priority_date,
                number,
            });
            if out.len() >= 12 {
                break;
            }
        }
        if out.len() >= 12 {
            break;
        }
    }
    out.sort_by(|a, b| b.priority_date.cmp(&a.priority_date));
    Ok(out)
}

fn is_candidate_patent(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    [
        "agentic",
        "autonomous",
        "unannounced",
        "stealth",
        "wearable",
        "neural",
        "generative",
        "device state",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

fn value_to_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::Object(map)) => map
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&hellip;", "…")
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// Jobs (career boards)
// ---------------------------------------------------------------------------

fn resolve_boards(company: &str, override_str: Option<&str>) -> Vec<String> {
    let lower = company.to_ascii_lowercase();
    if let Some(cfg) = override_str {
        for entry in cfg.split(',') {
            if let Some((name, spec)) = entry.split_once('=')
                && name.trim().eq_ignore_ascii_case(&lower)
            {
                return vec![spec.trim().to_string()];
            }
        }
    }
    if let Ok(env_cfg) = std::env::var("RADAR30_JOB_BOARDS")
        && let Some(cfg) = env_cfg.split(',').find_map(|entry| {
            entry
                .split_once('=')
                .filter(|(name, _)| name.trim().eq_ignore_ascii_case(&lower))
                .map(|(_, spec)| spec.trim().to_string())
        })
    {
        return vec![cfg];
    }
    if let Some(spec) = hint(BOARD_HINTS, company) {
        return vec![spec.to_string()];
    }
    let slug: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    vec![format!("greenhouse:{slug}"), format!("ashby:{slug}")]
}

fn fetch_jobs(
    board: &str,
    timeout: Duration,
    deadline: Instant,
) -> Result<Vec<JobItem>, HttpError> {
    let headers = [("User-Agent", BROWSER_UA), ("Accept", "application/json")];
    match board.split_once(':') {
        Some(("greenhouse", token)) => {
            let url =
                format!("https://boards-api.greenhouse.io/v1/boards/{token}/jobs?content=false");
            let data: Value = http::get_json(&url, &headers, timeout, deadline, 1)?;
            Ok(data["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|j| JobItem {
                    title: j["title"].as_str().unwrap_or("").to_string(),
                    url: j["absolute_url"].as_str().unwrap_or("").to_string(),
                    location: j["location"]["name"].as_str().unwrap_or("").to_string(),
                    team: j["departments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| d["name"].as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    published_at: j["updated_at"]
                        .as_str()
                        .or_else(|| j["first_published"].as_str())
                        .and_then(|s| s.get(..10))
                        .map(ToOwned::to_owned),
                    board: board.to_string(),
                    is_secret_signal: is_secret_role(j["title"].as_str().unwrap_or("")),
                })
                .filter(|j| !j.title.is_empty() && !j.url.is_empty())
                .collect())
        }
        Some(("ashby", token)) => {
            let url = format!("https://api.ashbyhq.com/posting-api/job-board/{token}");
            let data: Value = http::get_json(&url, &headers, timeout, deadline, 1)?;
            Ok(data["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|j| JobItem {
                    title: j["title"].as_str().unwrap_or("").to_string(),
                    url: j["jobUrl"]
                        .as_str()
                        .or_else(|| j["applyUrl"].as_str())
                        .unwrap_or("")
                        .to_string(),
                    location: j["location"].as_str().unwrap_or("").to_string(),
                    team: [j["department"].as_str(), j["team"].as_str()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                    published_at: j["publishedAt"]
                        .as_str()
                        .and_then(|s| s.get(..10))
                        .map(ToOwned::to_owned),
                    board: board.to_string(),
                    is_secret_signal: is_secret_role(j["title"].as_str().unwrap_or("")),
                })
                .filter(|j| !j.title.is_empty() && !j.url.is_empty())
                .collect())
        }
        Some(("lever", token)) => {
            let url = format!("https://api.lever.co/v0/postings/{token}?mode=json");
            let data: Value = http::get_json(&url, &headers, timeout, deadline, 1)?;
            Ok(data
                .as_array()
                .into_iter()
                .flatten()
                .map(|j| JobItem {
                    title: j["text"].as_str().unwrap_or("").to_string(),
                    url: j["hostUrl"].as_str().unwrap_or("").to_string(),
                    location: j["categories"]["location"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    team: j["categories"]["team"].as_str().unwrap_or("").to_string(),
                    published_at: j["createdAt"]
                        .as_u64()
                        .and_then(NasdaqPoint::ms_to_date)
                        .map(|d| d.to_string()),
                    board: board.to_string(),
                    is_secret_signal: is_secret_role(j["text"].as_str().unwrap_or("")),
                })
                .filter(|j| !j.title.is_empty() && !j.url.is_empty())
                .collect())
        }
        Some(("workday", tenant_and_site)) => {
            let mut parts = tenant_and_site.splitn(2, ':');
            let tenant = parts.next().unwrap_or("");
            let site = parts.next().unwrap_or("");
            let url =
                format!("https://{tenant}.wd1.myworkdayjobs.com/wday/cxs/{tenant}/{site}/jobs");
            let body = json!({"appliedFacets": {}, "limit": 20, "offset": 0, "searchText": ""});
            let data: Value = http::post_json(&url, &headers, &body, timeout, deadline, 1)?;
            Ok(data["jobPostings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|j| JobItem {
                    title: j["title"].as_str().unwrap_or("").to_string(),
                    url: format!(
                        "https://{tenant}.wd1.myworkdayjobs.com/en-US/{site}{}",
                        j["externalPath"].as_str().unwrap_or("")
                    ),
                    location: String::new(),
                    team: String::new(),
                    published_at: j["postedOn"]
                        .as_str()
                        .and_then(|s| s.get(..10))
                        .map(ToOwned::to_owned),
                    board: board.to_string(),
                    is_secret_signal: is_secret_role(j["title"].as_str().unwrap_or("")),
                })
                .filter(|j| !j.title.is_empty())
                .collect())
        }
        _ => Err(HttpError::Decode {
            detail: format!(
                "unknown board spec `{board}` (use greenhouse:<token>, ashby:<token>, lever:<token>, workday:<tenant>:<site>)"
            ),
        }),
    }
}

fn is_secret_role(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    [
        "stealth",
        "unannounced",
        "confidential",
        "new product",
        "0 to 1",
        "0→1",
        "agentic",
        "genai",
        "ai native",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

// ---------------------------------------------------------------------------
// Finance (Nasdaq keyless chart API, VOO as S&P 500 proxy)
// ---------------------------------------------------------------------------

struct NasdaqPoint;

impl NasdaqPoint {
    fn ms_to_date(ms: u64) -> Option<NaiveDate> {
        let secs = (ms / 1000) as i64;
        let dt = chrono::DateTime::from_timestamp(secs, 0)?;
        Some(dt.date_naive())
    }
}

fn fetch_finance(
    ticker: &str,
    window: DateWindow,
    timeout: Duration,
    deadline: Instant,
) -> Result<FinanceLine, HttpError> {
    let company_change = nasdaq_change(ticker, window, timeout, deadline)?;
    let benchmark_change = nasdaq_change(BENCHMARK_TICKER, window, timeout, deadline)
        .map(|(pct, _)| pct)
        .unwrap_or(0.0);
    let now = Utc::now();
    Ok(FinanceLine {
        ticker: ticker.to_ascii_uppercase(),
        benchmark_ticker: BENCHMARK_TICKER.to_string(),
        company_change_pct: company_change.0,
        benchmark_change_pct: benchmark_change,
        last_price: company_change.1,
        compared_days: (now.date_naive() - window.from).num_days().max(1),
    })
}

fn nasdaq_change(
    ticker: &str,
    window: DateWindow,
    timeout: Duration,
    deadline: Instant,
) -> Result<(f64, Option<String>), HttpError> {
    let asset_class = if matches!(ticker, "COMP" | "NDX" | "DJIA") {
        "index"
    } else if ["VOO", "SPY", "QQQ"].contains(&ticker) {
        "etf"
    } else {
        "stocks"
    };
    let url = format!(
        "{NASDAQ_CHART_URL}/{}/chart?assetclass={asset_class}",
        ticker.to_ascii_uppercase()
    );
    let headers = [("User-Agent", BROWSER_UA), ("Accept", "application/json")];
    let data: Value = http::get_json(&url, &headers, timeout, deadline, 1)?;
    parse_nasdaq_chart(&data, window)
}

fn parse_nasdaq_chart(
    data: &Value,
    window: DateWindow,
) -> Result<(f64, Option<String>), HttpError> {
    let points = data["data"]["chart"]
        .as_array()
        .ok_or_else(|| HttpError::Decode {
            detail: "Nasdaq response has no chart array".to_string(),
        })?;
    let mut in_window: Vec<(NaiveDate, f64)> = points
        .iter()
        .filter_map(|p| {
            let ms = p["x"].as_u64()?;
            let y = p["y"].as_f64()?;
            NasdaqPoint::ms_to_date(ms).map(|d| (d, y))
        })
        .filter(|(d, _)| *d >= window.from && *d <= window.to)
        .collect();
    in_window.sort_by_key(|(d, _)| *d);
    if in_window.len() < 2 {
        return Err(HttpError::Decode {
            detail: "fewer than 2 in-window price points".to_string(),
        });
    }
    let first = in_window.first().expect("len checked").1;
    let last = *in_window.last().expect("len checked");
    if first <= 0.0 {
        return Err(HttpError::Decode {
            detail: "invalid baseline price".to_string(),
        });
    }
    let pct = ((last.1 - first) / first) * 100.0;
    Ok((
        (pct * 100.0).round() / 100.0,
        data["data"]["lastSalePrice"]
            .as_str()
            .map(ToOwned::to_owned),
    ))
}

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------

pub fn render(report: &CompanyReport, format: crate::schema::EmitFormat) -> Result<String> {
    match format {
        crate::schema::EmitFormat::Json => Ok(serde_json::to_string_pretty(report)?),
        crate::schema::EmitFormat::Compact => Ok(render_text(report, false)),
        crate::schema::EmitFormat::Md => Ok(render_text(report, true)),
    }
}

fn render_text(report: &CompanyReport, markdown: bool) -> String {
    let h = |s: &str| {
        if markdown {
            format!("## {s}\n")
        } else {
            format!("{s}\n")
        }
    };
    let bullet = |s: &str| {
        if markdown {
            format!("- {s}\n")
        } else {
            format!("  - {s}\n")
        }
    };
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} ({} .. {})",
        if markdown {
            "# Company Intel:"
        } else {
            "# radar30 company:"
        },
        report.company,
        report.window.from,
        report.window.to
    );

    let _ = write!(out, "{}", h("Patent Digest (US publications)"));
    if report.patents.is_empty() {
        let _ = write!(out, "{}", bullet("No US publications found in the window."));
    } else {
        for p in &report.patents {
            let flag = if p.is_candidate_signal {
                " [candidate signal]"
            } else {
                ""
            };
            let date = p.priority_date.as_deref().unwrap_or("undated");
            let _ = write!(
                out,
                "{}",
                bullet(&format!(
                    "{} — {} ({}, {}){} — {}",
                    p.number,
                    p.title,
                    date,
                    if p.assignee.is_empty() {
                        "unknown assignee"
                    } else {
                        &p.assignee
                    },
                    flag,
                    p.url
                ))
            );
        }
    }
    let _ = writeln!(out);

    let _ = write!(out, "{}", h("Jobs Digest (careers pages)"));
    if report.jobs.is_empty() {
        let _ = write!(out, "{}", bullet("No postings found on configured boards."));
    } else {
        for j in report.jobs.iter().take(20) {
            let flag = if j.is_secret_signal {
                " [Jobs Page Secret Signal]"
            } else {
                ""
            };
            let _ = write!(
                out,
                "{}",
                bullet(&format!(
                    "{} — {} ({}, {}){} — {}",
                    j.title, j.board, j.location, j.team, flag, j.url
                ))
            );
        }
    }
    let _ = writeln!(out);

    let _ = write!(out, "{}", h("Finance line"));
    match &report.finance {
        Some(f) => {
            let _ = write!(
                out,
                "{}",
                bullet(&format!(
                    "{} {:+.1}% over {}d vs {} (S&P 500 proxy) {:+.1}%{}",
                    f.ticker,
                    f.company_change_pct,
                    f.compared_days,
                    f.benchmark_ticker,
                    f.benchmark_change_pct,
                    f.last_price
                        .as_deref()
                        .map(|p| format!(" · last {p}"))
                        .unwrap_or_default()
                ))
            );
        }
        None => {
            let _ = write!(
                out,
                "{}",
                bullet("No ticker hint; pass --ticker <SYMBOL> to enable.")
            );
        }
    }

    if !report.warnings.is_empty() {
        let _ = writeln!(out, "\nWarnings:");
        for w in &report.warnings {
            let _ = write!(out, "{}", bullet(w));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_patents_envelope_and_filters_non_us() {
        let raw = json!({"results":{"cluster":[{"result":[
            {"patent":{"title":"Agentic device state","priority_date":"2026-11-02","publication_number":"US20260312345A1","assignee":"Acme Inc."}},
            {"patent":{"title":"Foreign filing","priority_date":"2026-11-03","publication_number":"CN121499512B","assignee":"Acme"}}
        ]}]}});
        let parsed: PatentsEnvelope = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.results.cluster[0].result.len(), 2);
        assert_eq!(
            parsed.results.cluster[0].result[0]
                .patent
                .publication_number
                .as_deref(),
            Some("US20260312345A1")
        );
    }

    #[test]
    fn parses_nasdaq_chart_points_within_window() {
        let now = Utc::now().date_naive();
        let start = now - chrono::Duration::days(30);
        let raw = json!({"data":{"lastSalePrice":"$110.00","chart":[
            {"x": start.and_hms_opt(0,0,0).unwrap().and_utc().timestamp_millis(), "y":100.0},
            {"x": now.and_hms_opt(0,0,0).unwrap().and_utc().timestamp_millis(), "y":110.0}
        ]}});
        let window = DateWindow {
            from: start,
            to: now,
        };
        let (pct, price) = parse_nasdaq_chart(&raw, window).unwrap();
        assert_eq!(pct, 10.0);
        assert_eq!(price.as_deref(), Some("$110.00"));
    }

    #[test]
    fn detects_patent_and_role_secret_signals() {
        assert!(is_candidate_patent("Agentic control of device state"));
        assert!(!is_candidate_patent("A chair"));
        assert!(is_secret_role("Senior Engineer, Stealth AI Initiative"));
        assert!(!is_secret_role("Support Specialist"));
    }

    #[test]
    fn board_resolution_uses_curated_hints_and_overrides() {
        assert_eq!(resolve_boards("Stripe", None), vec!["greenhouse:stripe"]);
        assert_eq!(resolve_boards("Linear", None), vec!["ashby:linear"]);
        assert_eq!(
            resolve_boards("Acme", Some("acme=lever:acme,other=ashby:other")),
            vec!["lever:acme"]
        );
    }
}
