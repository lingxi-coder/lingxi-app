//! Hand-written case-insensitive subsequence matcher + ranked filter.
//!
//! DECISION D1 (M7-07): no fuzzy-match crate is a workspace dependency, and
//! adding one for v0.8.0 would violate the exact-pin discipline (design §2.2)
//! and YAGNI. This module is a small subsequence matcher: every `needle` char
//! must appear in `haystack` in order (case-insensitive). The score rewards
//! contiguous matches (fewer gaps = higher score). Good enough for the 99
//! short command names and a cwd listing. A real fuzzy crate can replace the
//! body behind `subsequence_match`'s signature in M8 if ranking quality ever
//! matters.

/// Case-insensitive subsequence test. Returns `Some(score)` if every char of
/// `needle` appears in `haystack` in order, else `None`. Higher score is a
/// better match; score is `-(gap_count)` so a contiguous run scores `0` and
/// each skipped haystack char between matches subtracts one. An empty needle
/// matches everything with score `0`.
#[must_use]
pub fn subsequence_match(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().flat_map(char::to_lowercase).collect();
    let mut gaps: i32 = 0;
    let mut hi = 0usize;
    let mut last_matched: Option<usize> = None;
    for nc in needle.chars().flat_map(char::to_lowercase) {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi] == nc {
                if let Some(prev) = last_matched {
                    gaps += i32::try_from(hi - prev - 1).unwrap_or(i32::MAX);
                }
                last_matched = Some(hi);
                hi += 1;
                found = true;
                break;
            }
            hi += 1;
        }
        if !found {
            return None;
        }
    }
    Some(-gaps)
}

/// Filter `candidates` to those matching `needle`, ranked best-first.
/// Sort key: score descending, then shorter candidate, then ASCII ascending —
/// deterministic so snapshots and behavior tests are stable.
#[must_use]
pub fn filtered_ranked<'a>(needle: &str, candidates: &'a [String]) -> Vec<&'a str> {
    if needle.is_empty() {
        // (cp-02) A bare `/` (empty filter) lists every candidate
        // alphabetically, case-insensitive — claude-code's `localeCompare`
        // ordering — NOT by name length.
        let mut all: Vec<&'a str> = candidates.iter().map(String::as_str).collect();
        all.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
        return all;
    }
    let mut scored: Vec<(i32, &'a str)> = candidates
        .iter()
        .filter_map(|c| subsequence_match(needle, c).map(|s| (s, c.as_str())))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(b.1))
    });
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_matches_everything_with_zero_score() {
        assert_eq!(subsequence_match("", "anything"), Some(0));
    }

    #[test]
    fn exact_prefix_outranks_scattered() {
        let contig = subsequence_match("co", "compact").expect("contiguous matches");
        let scattered = subsequence_match("co", "context").expect("also matches");
        // Both match; "compact" has the run "co" adjacent at the very start,
        // "context" also starts "co" — tie on gaps, so ASCII order in the
        // caller decides. Here assert both are Some and contiguous beats gappy:
        let gappy = subsequence_match("ct", "context").expect("c..t matches");
        assert!(contig >= gappy, "contiguous run must score >= gappy run");
        let _ = scattered;
    }

    #[test]
    fn non_subsequence_returns_none() {
        assert_eq!(subsequence_match("xyz", "compact"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(subsequence_match("CMP", "compact").is_some());
    }

    #[test]
    fn filtered_ranked_orders_contiguous_first() {
        let cands = vec![
            "context".to_string(),
            "compact".to_string(),
            "copy".to_string(),
        ];
        let out = filtered_ranked("co", &cands);
        // All three contain "co"; "copy" and "compact" and "context" all start
        // with "co" (gap 0). Tie broken by shorter then ASCII → copy, compact,
        // context. Just assert ordering is deterministic and copy is first.
        assert_eq!(out, vec!["copy", "compact", "context"]);
    }

    #[test]
    fn filtered_ranked_drops_non_matches() {
        let cands = vec!["help".to_string(), "exit".to_string()];
        assert_eq!(filtered_ranked("zz", &cands), Vec::<&str>::new());
    }
}
