//! Greedy text-similarity clustering and MMR diverse representative selection.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::query::tokenize;
use crate::schema::{Candidate, Cluster, ClusterUncertainty, QueryPlan};

/// Prepared token + bigram representation for fast similarity comparisons.
struct PreparedText {
    tokens: HashSet<String>,
    bigrams: HashSet<String>,
}

impl PreparedText {
    fn from_candidate(c: &Candidate) -> Self {
        let combined = format!("{} {}", c.title, c.snippet);
        let raw_tokens: Vec<String> = tokenize(&combined)
            .into_iter()
            .filter(|t| t.len() >= 3)
            .collect();
        let tokens: HashSet<String> = raw_tokens.iter().cloned().collect();
        let bigrams: HashSet<String> = raw_tokens
            .windows(2)
            .map(|w| format!("{} {}", w[0], w[1]))
            .collect();
        Self { tokens, bigrams }
    }
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 { 0.0 } else { inter / union }
}

fn similarity(a: &PreparedText, b: &PreparedText) -> f64 {
    let tok = jaccard(&a.tokens, &b.tokens);
    let bi = jaccard(&a.bigrams, &b.bigrams);
    0.70 * tok + 0.30 * bi
}

/// Select up to `limit` diverse representative candidate IDs using Maximal Marginal Relevance.
fn mmr_representatives(
    group: &[&Candidate],
    cache: &HashMap<String, PreparedText>,
    limit: usize,
) -> Vec<String> {
    const LAMBDA: f64 = 0.75;
    let mut remaining: Vec<&Candidate> = group.to_vec();
    let mut selected: Vec<&Candidate> = Vec::new();

    while !remaining.is_empty() && selected.len() < limit {
        if selected.is_empty() {
            let best = remaining.remove(0);
            selected.push(best);
            continue;
        }
        let mut best_idx = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        for (idx, cand) in remaining.iter().enumerate() {
            let prep = &cache[&cand.candidate_id];
            let max_sim = selected
                .iter()
                .map(|s| similarity(prep, &cache[&s.candidate_id]))
                .fold(0.0_f64, f64::max);
            let mmr = (LAMBDA * cand.final_score) - ((1.0 - LAMBDA) * max_sim * 100.0);
            let adjusted = if cand.out_of_window {
                mmr - 1000.0
            } else {
                mmr
            };
            if adjusted > best_score {
                best_score = adjusted;
                best_idx = idx;
            }
        }
        selected.push(remaining.remove(best_idx));
    }

    selected
        .into_iter()
        .map(|c| c.candidate_id.clone())
        .collect()
}

/// Cluster ranked candidates into corroborated story clusters, mutating `candidate.cluster_id`.
pub fn cluster_candidates(candidates: &mut [Candidate], plan: &QueryPlan) -> Vec<Cluster> {
    if candidates.is_empty() {
        return Vec::new();
    }

    if !plan.intent.is_clusterable() {
        let mut clusters = Vec::with_capacity(candidates.len());
        for (idx, cand) in candidates.iter_mut().enumerate() {
            let cid = format!("cluster-{}", idx + 1);
            cand.cluster_id = Some(cid.clone());
            clusters.push(Cluster {
                cluster_id: cid,
                title: cand.title.clone(),
                candidate_ids: vec![cand.candidate_id.clone()],
                representative_ids: vec![cand.candidate_id.clone()],
                sources: cand.sources.clone(),
                score: cand.final_score,
                uncertainty: if cand.sources.len() <= 1 {
                    Some(ClusterUncertainty::SingleSource)
                } else {
                    None
                },
            });
        }
        return clusters;
    }

    let cache: HashMap<String, PreparedText> = candidates
        .iter()
        .map(|c| (c.candidate_id.clone(), PreparedText::from_candidate(c)))
        .collect();

    let threshold = plan.intent.cluster_threshold();
    let mut groups: Vec<Vec<usize>> = Vec::new();

    for (idx, cand) in candidates.iter().enumerate() {
        let prep = &cache[&cand.candidate_id];
        let mut assigned = false;
        for group in &mut groups {
            let leader = &candidates[group[0]];
            if similarity(prep, &cache[&leader.candidate_id]) >= threshold {
                group.push(idx);
                assigned = true;
                break;
            }
        }
        if !assigned {
            groups.push(vec![idx]);
        }
    }

    let mut clusters = Vec::with_capacity(groups.len());
    for (g_idx, mut group) in groups.into_iter().enumerate() {
        group.sort_by(|&a, &b| {
            candidates[a]
                .out_of_window
                .cmp(&candidates[b].out_of_window)
                .then_with(|| {
                    candidates[b]
                        .final_score
                        .total_cmp(&candidates[a].final_score)
                })
        });

        let cluster_id = format!("cluster-{}", g_idx + 1);
        let refs: Vec<&Candidate> = group.iter().map(|&i| &candidates[i]).collect();
        let reps = mmr_representatives(&refs, &cache, 3);
        let sources: BTreeSet<_> = refs
            .iter()
            .flat_map(|c| c.sources.iter().copied())
            .collect();
        let title = refs[0].title.clone();
        let score = refs.iter().map(|c| c.final_score).fold(0.0_f64, f64::max);
        let uncertainty = if sources.len() <= 1 && refs.len() <= 1 {
            Some(ClusterUncertainty::SingleSource)
        } else if refs.iter().all(|c| c.engagement.primary_metric() < 5.0) {
            Some(ClusterUncertainty::ThinEvidence)
        } else {
            None
        };
        let candidate_ids: Vec<String> = refs.iter().map(|c| c.candidate_id.clone()).collect();

        for &i in &group {
            candidates[i].cluster_id = Some(cluster_id.clone());
        }

        clusters.push(Cluster {
            cluster_id,
            title,
            candidate_ids,
            representative_ids: reps,
            sources: sources.into_iter().collect(),
            score,
            uncertainty,
        });
    }

    clusters.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.title.cmp(&b.title))
    });
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::build_plan;
    use crate::schema::{Engagement, SourceId};
    use std::collections::BTreeMap;

    fn make_cand(id: &str, title: &str, src: SourceId, score: f64) -> Candidate {
        Candidate {
            candidate_id: id.to_string(),
            canonical_key: id.to_string(),
            primary_source: src,
            sources: vec![src],
            title: title.to_string(),
            url: format!("https://example.com/{id}"),
            snippet: title.to_string(),
            published_at: Some("2026-11-18".to_string()),
            out_of_window: false,
            subquery_labels: vec!["primary".to_string()],
            native_ranks: BTreeMap::new(),
            local_relevance: 0.9,
            entity_grounded: true,
            freshness: 95,
            engagement: Engagement {
                score: Some(100),
                ..Default::default()
            },
            engagement_score: 0.7,
            source_quality: 0.8,
            rrf_score: 0.02,
            final_score: score,
            cluster_id: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn clusters_similar_stories_across_sources() {
        let plan = build_plan("Rust release", &[SourceId::HackerNews, SourceId::Reddit]);
        let mut cands = vec![
            make_cand(
                "cand-1",
                "Rust 1.100 compiler release notes announced",
                SourceId::HackerNews,
                88.0,
            ),
            make_cand(
                "cand-2",
                "Rust 1.100 compiler release notes discussion",
                SourceId::Reddit,
                81.0,
            ),
            make_cand(
                "cand-3",
                "Totally different Python async tutorial",
                SourceId::DevTo,
                40.0,
            ),
        ];
        let clusters = cluster_candidates(&mut cands, &plan);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].candidate_ids.len(), 2);
        assert_eq!(clusters[0].sources.len(), 2);
        assert_eq!(clusters[0].uncertainty, None);
    }
}
