//! `companyAnnouncements` startup-message selection.
//!
//! 1:1 port of claude-code 2.1.207's announcement helpers `jxo` (select) and
//! `oip` (gate):
//!
//! ```js
//! function oip(){let e=Wn().companyAnnouncements;return!!e&&e.some((t)=>t)}
//! function jxo(e){
//!   if(Wxo!==null)return Wxo;                       // process memo
//!   let t=(Wn().companyAnnouncements??[]).filter((n)=>n);   // non-empty only
//!   if(t.length===0)return null;
//!   let r=St().numStartups===1 ? t[0] : t[Math.floor(Math.random()*t.length)];
//!   if(!r)return null;
//!   if(e)Wxo=r;                                      // memoize when asked
//!   return r
//! }
//! ```
//!
//! The `companyAnnouncements` settings array is read from the merged
//! [`crate::settings::schema::SettingsJson`]. Non-empty strings are kept; the
//! first is chosen deterministically on the very first launch
//! (`numStartups === 1`), otherwise one is chosen uniformly at random. A
//! process-level memo ([`AnnouncementMemo`]) keeps the SAME announcement stable
//! across re-renders within one process (the `Wxo` cache; the `e` flag on `jxo`).
//!
//! NOTE (remainder): the `numStartups` counter and the TUI startup-header render
//! site are the composition-root/TUI wiring for this feature — this module ports
//! the selection + gate logic and the memo; the caller supplies `num_startups`
//! (sourced from the local config state) and renders the returned string as the
//! dim startup tip (binary `Jf_={tip,color:"dim"}`).

use std::sync::Mutex;

/// `oip()` — is there at least one non-empty announcement to show?
///
/// `!!e && e.some((t)=>t)`: `Some` list with at least one non-empty string.
#[must_use]
pub fn has_company_announcement(announcements: Option<&[String]>) -> bool {
    announcements.is_some_and(|a| a.iter().any(|s| !s.is_empty()))
}

/// Pure selection with an injected random index (the `Math.floor(random*len)`
/// value). Filters to non-empty strings first (`.filter((n)=>n)`), returns the
/// first on `num_startups == 1`, else the entry at `random_index % len`.
///
/// Returns `None` when there are no non-empty announcements.
#[must_use]
pub fn select_company_announcement_at(
    announcements: Option<&[String]>,
    num_startups: u64,
    random_index: usize,
) -> Option<String> {
    let non_empty: Vec<&String> = announcements?.iter().filter(|s| !s.is_empty()).collect();
    if non_empty.is_empty() {
        return None;
    }
    let chosen = if num_startups == 1 {
        non_empty[0]
    } else {
        non_empty[random_index % non_empty.len()]
    };
    Some(chosen.clone())
}

/// `jxo`-equivalent selection: picks the first announcement on the first launch
/// (`num_startups == 1`) else a uniformly-random one. Dependency-free entropy
/// (process/thread nanos) drives the random branch — the exact index is not a
/// parity-observable value (any configured announcement is valid to show), and
/// the deterministic first-launch branch is fully specified.
#[must_use]
pub fn select_company_announcement(
    announcements: Option<&[String]>,
    num_startups: u64,
) -> Option<String> {
    select_company_announcement_at(announcements, num_startups, random_seed_index())
}

/// Process-level memo mirroring binary `Wxo`: the first memoized selection is
/// returned verbatim on every subsequent call, keeping the announcement stable
/// across re-renders within one process.
#[derive(Debug, Default)]
pub struct AnnouncementMemo {
    cell: Mutex<Option<Option<String>>>,
}

impl AnnouncementMemo {
    /// Construct an empty memo.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cell: Mutex::new(None),
        }
    }

    /// `jxo(memoize)`: return the memoized selection if present, otherwise
    /// compute a fresh selection and — when `memoize` is `true` — cache it.
    ///
    /// Mirrors `if(Wxo!==null)return Wxo; …; if(e)Wxo=r; return r`.
    pub fn select(
        &self,
        announcements: Option<&[String]>,
        num_startups: u64,
        memoize: bool,
    ) -> Option<String> {
        let mut guard = self.cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
        let selected = select_company_announcement(announcements, num_startups);
        if memoize {
            *guard = Some(selected.clone());
        }
        selected
    }
}

/// Dependency-free entropy for the uniform-random branch. Not cryptographic;
/// used only to pick which of several equally-valid announcements to show.
fn random_seed_index() -> usize {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    nanos as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn gate_true_only_when_a_non_empty_entry_exists() {
        assert!(!has_company_announcement(None));
        assert!(!has_company_announcement(Some(&[])));
        assert!(!has_company_announcement(Some(&v(&["", ""]))));
        assert!(has_company_announcement(Some(&v(&["", "hi"]))));
    }

    #[test]
    fn first_launch_is_deterministic_first_non_empty() {
        // num_startups == 1 → always the FIRST non-empty entry, regardless of the
        // random index.
        let a = v(&["", "first", "second"]);
        assert_eq!(
            select_company_announcement_at(Some(&a), 1, 999),
            Some("first".to_string())
        );
    }

    #[test]
    fn empty_and_all_empty_yield_none() {
        assert_eq!(select_company_announcement_at(None, 1, 0), None);
        assert_eq!(
            select_company_announcement_at(Some(&v(&[])), 5, 0),
            None
        );
        assert_eq!(
            select_company_announcement_at(Some(&v(&["", ""])), 5, 0),
            None
        );
    }

    #[test]
    fn random_branch_picks_a_non_empty_member() {
        // num_startups != 1 → index into the FILTERED (non-empty) list.
        let a = v(&["", "a", "b", "c"]);
        // Filtered list is [a,b,c]; index 4 % 3 == 1 → "b".
        assert_eq!(
            select_company_announcement_at(Some(&a), 7, 4),
            Some("b".to_string())
        );
        // Whatever the index, the result is always one of the non-empty members.
        for idx in 0..20usize {
            let got = select_company_announcement_at(Some(&a), 7, idx).unwrap();
            assert!(["a", "b", "c"].contains(&got.as_str()), "got {got}");
        }
    }

    #[test]
    fn memo_keeps_the_selection_stable_across_calls() {
        let memo = AnnouncementMemo::new();
        let a = v(&["only"]);
        let first = memo.select(Some(&a), 3, true);
        assert_eq!(first, Some("only".to_string()));
        // Even if a later call passes different inputs, the memoized value wins.
        let second = memo.select(Some(&v(&["different"])), 3, true);
        assert_eq!(second, Some("only".to_string()));
    }

    #[test]
    fn memo_does_not_cache_when_memoize_false() {
        let memo = AnnouncementMemo::new();
        // memoize=false must not populate the cache (binary: `if(e)Wxo=r`).
        let _ = memo.select(Some(&v(&["a"])), 1, false);
        // A subsequent memoize=true call then decides the cached value.
        let cached = memo.select(Some(&v(&["b"])), 1, true);
        assert_eq!(cached, Some("b".to_string()));
    }
}
