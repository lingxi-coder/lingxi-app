//! Relevance ranking with `#![deny(clippy::float_arithmetic)]`.

#![deny(clippy::float_arithmetic)]

use lingxi_protocol::MemoryEntry;

/// Inputs to `find_relevant`. Bundled so the public signature stays
/// stable as we add fields (e.g. recent tools, agent type) in M5.
#[derive(Debug)]
pub struct RelevanceInputs<'a> {
    /// Prompt text — keyword overlap basis.
    pub prompt: &'a str,
    /// Top-k cutoff. `None` → `DEFAULT_RELEVANT_MEMORIES` (5).
    pub k: Option<usize>,
    /// Whether team boost applies (mirrors `settings.team_memory.enabled`).
    pub team_boost_enabled: bool,
}

/// Tokenize `s` into lowercase ASCII word tokens (`[a-z0-9_]+`).
///
/// Stop-words are NOT removed (claude-code default). Tokens are
/// deduplicated for set-style Jaccard.
#[must_use]
pub fn tokenize(s: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut current = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            out.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.insert(current);
    }
    out
}

/// Integer Jaccard similarity in basis points (×`10_000`).
///
/// `intersection * 10_000 / union` in u64. Returns `0` when union is `0`
/// (i.e. both inputs are empty).
#[must_use]
pub fn jaccard_bps(prompt: &str, entry_body: &str) -> u64 {
    let a = tokenize(prompt);
    let b = tokenize(entry_body);
    let intersection = a.intersection(&b).count() as u64;
    let union = (a.len() as u64) + (b.len() as u64) - intersection;
    if union == 0 {
        return 0;
    }
    intersection * 10_000 / union
}

/// Placeholder; real impl in Task 9.
#[must_use]
pub fn find_relevant(
    _entries: &[MemoryEntry],
    _inputs: &RelevanceInputs<'_>,
) -> Vec<MemoryEntry> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_lowercases_and_splits_on_punctuation() {
        let t = tokenize("Hello, World! Foo_bar 123.");
        let v: Vec<_> = t.iter().cloned().collect();
        assert_eq!(v, vec!["123".to_string(), "foo_bar".into(), "hello".into(), "world".into()]);
    }

    #[test]
    fn jaccard_identical_strings_returns_10000() {
        assert_eq!(jaccard_bps("alpha beta", "alpha beta"), 10_000);
    }

    #[test]
    fn jaccard_disjoint_returns_zero() {
        assert_eq!(jaccard_bps("alpha", "gamma"), 0);
    }

    #[test]
    fn jaccard_partial_overlap() {
        // {a,b,c} ∩ {b,c,d} = {b,c} → 2; union = {a,b,c,d} → 4. 2*10000/4 = 5000.
        assert_eq!(jaccard_bps("a b c", "b c d"), 5_000);
    }

    #[test]
    fn jaccard_empty_inputs_yields_zero() {
        assert_eq!(jaccard_bps("", ""), 0);
        assert_eq!(jaccard_bps("alpha", ""), 0);
    }

    #[test]
    fn jaccard_deterministic() {
        for _ in 0..5 {
            assert_eq!(jaccard_bps("alpha beta gamma", "beta gamma delta"), jaccard_bps("alpha beta gamma", "beta gamma delta"));
        }
    }
}
