//! End-to-end research pipeline: plan -> parallel fetch -> score -> RRF fuse -> cluster.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use chrono::{Days, NaiveDate, Utc};

use crate::cluster::cluster_candidates;
use crate::fusion::{RankedStream, fuse_streams};
use crate::query::build_plan;
use crate::schema::{
    DateWindow, Depth, RESEARCH_SCHEMA, ResearchReport, SourceId, SourceOutcome, SourceState,
};
use crate::signals::annotate_and_sort_stream;
use crate::sources::{FetchOutcome, SourceFetcher};

/// Options controlling one `pipeline::run` execution.
#[derive(Clone, Debug)]
pub struct RunOptions {
    pub topic: String,
    pub depth: Depth,
    pub window: DateWindow,
    pub sources: Vec<SourceId>,
    pub timeout: Duration,
}

impl RunOptions {
    pub fn resolve_window(
        days: u64,
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
    ) -> Result<DateWindow> {
        let today = Utc::now().date_naive();
        let end = to.unwrap_or(today);
        let start = match from {
            Some(d) => d,
            None => end
                .checked_sub_days(Days::new(days.clamp(1, 365)))
                .unwrap_or(end),
        };
        if start > end {
            bail!("invalid date window: --from ({start}) is after --to ({end})");
        }
        Ok(DateWindow {
            from: start,
            to: end,
        })
    }
}

/// Execute the complete research pipeline using the provided `SourceFetcher`.
pub fn run(opts: &RunOptions, fetcher: &dyn SourceFetcher) -> Result<ResearchReport> {
    let topic = opts.topic.trim();
    if topic.is_empty() {
        bail!("topic cannot be empty");
    }
    if opts.sources.is_empty() {
        bail!("at least one source must be enabled");
    }

    let plan = build_plan(topic, &opts.sources);
    let deadline = Instant::now() + opts.timeout;

    // Fan out every (subquery, source) pair concurrently using scoped OS threads.
    let jobs: Vec<(String, String, String, f64, SourceId)> = plan
        .subqueries
        .iter()
        .flat_map(|sq| {
            sq.sources.iter().map(move |&src| {
                (
                    sq.label.clone(),
                    sq.search_query.clone(),
                    sq.ranking_query.clone(),
                    sq.weight,
                    src,
                )
            })
        })
        .collect();

    let raw_results: Vec<(String, String, f64, FetchOutcome)> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(label, search_q, rank_q, weight, src)| {
                let window = opts.window;
                let depth = opts.depth;
                s.spawn(move || {
                    let outcome = fetcher.fetch(src, &search_q, window, depth, deadline);
                    (label, rank_q, weight, outcome)
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().expect("source worker thread panicked"))
            .collect()
    });

    // Aggregate per-source telemetry and annotate each subquery stream.
    let mut per_source: BTreeMap<SourceId, SourceOutcome> = BTreeMap::new();
    let mut streams: Vec<RankedStream> = Vec::with_capacity(raw_results.len());
    let mut total_fetched = 0usize;

    for (label, ranking_query, weight, mut outcome) in raw_results {
        total_fetched += outcome.items.len();

        let entry = per_source.entry(outcome.source).or_insert(SourceOutcome {
            source: outcome.source,
            state: outcome.state,
            item_count: 0,
            elapsed_ms: 0,
            detail: outcome.detail.clone(),
        });
        entry.item_count += outcome.items.len();
        entry.elapsed_ms = entry.elapsed_ms.max(outcome.elapsed_ms);
        entry.state = merge_source_state(entry.state, outcome.state);
        if entry.detail.is_none() {
            entry.detail = outcome.detail.clone();
        }

        if !outcome.items.is_empty() {
            annotate_and_sort_stream(&mut outcome.items, &ranking_query, &plan, opts.window);
            streams.push(RankedStream {
                subquery_label: label,
                source: outcome.source,
                weight,
                items: outcome.items,
            });
        }
    }

    let mut candidates = fuse_streams(&streams, &plan, opts.window, opts.depth);
    let clusters = cluster_candidates(&mut candidates, &plan);

    let source_outcomes: Vec<SourceOutcome> = per_source.into_values().collect();
    let mut warnings = Vec::new();
    for o in &source_outcomes {
        if !matches!(o.state, SourceState::Ok | SourceState::NoResults) {
            let reason = o.detail.as_deref().unwrap_or("degraded");
            warnings.push(format!("{}: {:?} ({reason})", o.source.as_str(), o.state));
        }
    }

    Ok(ResearchReport {
        schema: RESEARCH_SCHEMA.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        topic: topic.to_string(),
        primary_entity: plan.primary_entity,
        intent: plan.intent,
        depth: opts.depth,
        window: opts.window,
        generated_at: Utc::now().to_rfc3339(),
        total_fetched,
        candidates,
        clusters,
        source_outcomes,
        warnings,
    })
}

fn merge_source_state(a: SourceState, b: SourceState) -> SourceState {
    match (a, b) {
        (SourceState::Ok, SourceState::Ok) => SourceState::Ok,
        (SourceState::Ok, SourceState::NoResults) | (SourceState::NoResults, SourceState::Ok) => {
            SourceState::Ok
        }
        (SourceState::Ok, _) | (_, SourceState::Ok) => SourceState::Partial,
        (SourceState::Partial, _) | (_, SourceState::Partial) => SourceState::Partial,
        (SourceState::NoResults, other) => other,
        (other, SourceState::NoResults) => other,
        (first, _) => first,
    }
}
