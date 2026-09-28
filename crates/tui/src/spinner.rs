//! Spinner verb pool + per-turn sampling + todo-leader-verb resolution.
//!
//! Ported from claude-code's `src/constants/spinnerVerbs.ts` (187 entries) and
//! `Spinner.tsx:162,169` via the deleted iocraft `tui/src/components/spinner.rs`
//! (git ref `f4ddad16f`, path `crates/tui/src/components/spinner.rs`).
//! `ChatWidget::spinner_text` (`chat_widget.rs`) samples one verb from
//! [`SPINNER_VERBS`] once per turn (on `TurnEvent::TurnStarted`) and shows it
//! unless a tool activity label or a `TodoWrite` `activeForm` (via
//! [`todo_leader_verb`]) takes precedence.
//!
//! NOT ported: claude-code's user-configurable `spinnerVerbs` `{mode, verbs}`
//! setting (the iocraft oracle's `resolve_spinner_verbs`/`SpinnerVerbsSetting`/
//! `SpinnerVerbsMode`, honoring `'replace'`/`'append'`). This backend's
//! settings reader has no `spinnerVerbs` field to resolve, so porting that
//! plumbing now would be a config seam with no data source. Wire it in if/when
//! the TUI settings reader grows that field; until then every caller uses the
//! built-in [`SPINNER_VERBS`] pool directly.

use tui_core::message::CurrentTodo;

/// claude-code's full spinner verb pool — all 187 entries in source order,
/// byte-for-byte from `claude-code/src/constants/spinnerVerbs.ts:17-203`
/// (apostrophes and diacritics preserved exactly: `Beboppin'`, `Flambéing`,
/// `Sautéing`, `Philosophising`, `Dilly-dallying`, …).
pub const SPINNER_VERBS: &[&str] = &[
    "Accomplishing",
    "Actioning",
    "Actualizing",
    "Architecting",
    "Baking",
    "Beaming",
    "Beboppin'",
    "Befuddling",
    "Billowing",
    "Blanching",
    "Bloviating",
    "Boogieing",
    "Boondoggling",
    "Booping",
    "Bootstrapping",
    "Brewing",
    "Bunning",
    "Burrowing",
    "Calculating",
    "Canoodling",
    "Caramelizing",
    "Cascading",
    "Catapulting",
    "Cerebrating",
    "Channeling",
    "Channelling",
    "Choreographing",
    "Churning",
    "Clauding",
    "Coalescing",
    "Cogitating",
    "Combobulating",
    "Composing",
    "Computing",
    "Concocting",
    "Considering",
    "Contemplating",
    "Cooking",
    "Crafting",
    "Creating",
    "Crunching",
    "Crystallizing",
    "Cultivating",
    "Deciphering",
    "Deliberating",
    "Determining",
    "Dilly-dallying",
    "Discombobulating",
    "Doing",
    "Doodling",
    "Drizzling",
    "Ebbing",
    "Effecting",
    "Elucidating",
    "Embellishing",
    "Enchanting",
    "Envisioning",
    "Evaporating",
    "Fermenting",
    "Fiddle-faddling",
    "Finagling",
    "Flambéing",
    "Flibbertigibbeting",
    "Flowing",
    "Flummoxing",
    "Fluttering",
    "Forging",
    "Forming",
    "Frolicking",
    "Frosting",
    "Gallivanting",
    "Galloping",
    "Garnishing",
    "Generating",
    "Gesticulating",
    "Germinating",
    "Gitifying",
    "Grooving",
    "Gusting",
    "Harmonizing",
    "Hashing",
    "Hatching",
    "Herding",
    "Honking",
    "Hullaballooing",
    "Hyperspacing",
    "Ideating",
    "Imagining",
    "Improvising",
    "Incubating",
    "Inferring",
    "Infusing",
    "Ionizing",
    "Jitterbugging",
    "Julienning",
    "Kneading",
    "Leavening",
    "Levitating",
    "Lollygagging",
    "Manifesting",
    "Marinating",
    "Meandering",
    "Metamorphosing",
    "Misting",
    "Moonwalking",
    "Moseying",
    "Mulling",
    "Mustering",
    "Musing",
    "Nebulizing",
    "Nesting",
    "Newspapering",
    "Noodling",
    "Nucleating",
    "Orbiting",
    "Orchestrating",
    "Osmosing",
    "Perambulating",
    "Percolating",
    "Perusing",
    "Philosophising",
    "Photosynthesizing",
    "Pollinating",
    "Pondering",
    "Pontificating",
    "Pouncing",
    "Precipitating",
    "Prestidigitating",
    "Processing",
    "Proofing",
    "Propagating",
    "Puttering",
    "Puzzling",
    "Quantumizing",
    "Razzle-dazzling",
    "Razzmatazzing",
    "Recombobulating",
    "Reticulating",
    "Roosting",
    "Ruminating",
    "Sautéing",
    "Scampering",
    "Schlepping",
    "Scurrying",
    "Seasoning",
    "Shenaniganing",
    "Shimmying",
    "Simmering",
    "Skedaddling",
    "Sketching",
    "Slithering",
    "Smooshing",
    "Sock-hopping",
    "Spelunking",
    "Spinning",
    "Sprouting",
    "Stewing",
    "Sublimating",
    "Swirling",
    "Swooping",
    "Symbioting",
    "Synthesizing",
    "Tempering",
    "Thinking",
    "Thundering",
    "Tinkering",
    "Tomfoolering",
    "Topsy-turvying",
    "Transfiguring",
    "Transmuting",
    "Twisting",
    "Undulating",
    "Unfurling",
    "Unravelling",
    "Vibing",
    "Waddling",
    "Wandering",
    "Warping",
    "Whatchamacalliting",
    "Whirlpooling",
    "Whirring",
    "Whisking",
    "Wibbling",
    "Working",
    "Wrangling",
    "Zesting",
    "Zigzagging",
];

/// Pick a pool index for a pool of `len` verbs, given a `seed`. claude-code
/// chooses a random start verb once per turn (`useState(() => sample(...))`,
/// `Math.floor(Math.random() * verbs.length)`); this is the seedable Rust
/// equivalent so tests are deterministic. Returns `0` for an empty pool
/// (defensive — the pool is never empty in practice).
///
/// Uses a self-contained `splitmix64` step (no external RNG crate, mirroring
/// the deleted iocraft `initial_verb_index`) to turn the seed into a
/// uniformly-distributed index in `0..len`.
#[must_use]
pub fn sample_index(len: usize, seed: u64) -> usize {
    if len == 0 {
        return 0;
    }
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // `z % len` is always `< len <= usize::MAX`, so the conversion never
    // fails; computing the modulus as `u64` avoids any lossy cast.
    let len_u64 = len as u64;
    usize::try_from(z % len_u64).unwrap_or(0)
}

/// A non-deterministic seed for a live per-turn sample, derived from the wall
/// clock (successive turns land on different verbs). Tests use a fixed seed
/// via [`sample_index`] directly.
///
/// The `u128 → u64` truncation is intentional: any 64 bits of the nanosecond
/// clock make an adequate one-shot RNG seed.
#[allow(clippy::cast_possible_truncation)]
fn live_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Get the verb at `index` from the built-in [`SPINNER_VERBS`] pool, wrapping
/// modulo the pool length. (The pool is non-empty.)
#[must_use]
pub fn pool_verb_at_index(index: usize) -> &'static str {
    SPINNER_VERBS[index % SPINNER_VERBS.len()]
}

/// Sample one verb from [`SPINNER_VERBS`], seeded from the wall clock. Call
/// once per turn (`TurnEvent::TurnStarted`) and hold the result for the
/// turn's lifetime — claude-code picks one verb per turn and does not rotate.
#[must_use]
pub fn sample_verb() -> &'static str {
    pool_verb_at_index(sample_index(SPINNER_VERBS.len(), live_seed()))
}

/// Resolve the active todo's leader verb, mirroring claude-code's
/// `Spinner.tsx:169` (`currentTodo?.activeForm ?? currentTodo?.subject`) and
/// the deleted iocraft `todo_leader_verb`. The text is used VERBATIM — it is
/// NOT matched against [`SPINNER_VERBS`]; whatever the model wrote in the
/// todo's `activeForm`/`subject` is shown directly (e.g. "Running tests").
///
/// Returns `None` when there is no current todo (or both fields are empty),
/// so the caller falls back to the tool activity label / sampled pool verb.
#[must_use]
pub fn todo_leader_verb(current_todo: Option<&CurrentTodo>) -> Option<String> {
    let todo = current_todo?;
    todo.active_form
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(todo.subject.clone()).filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_pool_has_187_verbs_in_source_order() {
        assert_eq!(SPINNER_VERBS.len(), 187);
        assert_eq!(SPINNER_VERBS[0], "Accomplishing");
        assert_eq!(SPINNER_VERBS[186], "Zigzagging");
        assert_eq!(SPINNER_VERBS[6], "Beboppin'");
        assert!(SPINNER_VERBS.contains(&"Flambéing"));
        assert!(SPINNER_VERBS.contains(&"Sautéing"));
        assert!(SPINNER_VERBS.contains(&"Philosophising"));
        assert!(SPINNER_VERBS.contains(&"Dilly-dallying"));
        assert!(SPINNER_VERBS.contains(&"Razzle-dazzling"));
        assert!(SPINNER_VERBS.contains(&"Sock-hopping"));
        assert!(SPINNER_VERBS.contains(&"Topsy-turvying"));
        assert!(SPINNER_VERBS.iter().all(|v| !v.is_empty()));
        let mut sorted = SPINNER_VERBS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SPINNER_VERBS.len(), "pool has duplicates");
    }

    #[test]
    fn sample_index_in_bounds_and_deterministic() {
        let len = SPINNER_VERBS.len();
        for seed in 0..10_000u64 {
            let idx = sample_index(len, seed);
            assert!(idx < len, "idx {idx} out of bounds for seed {seed}");
        }
        assert_eq!(sample_index(len, 42), sample_index(len, 42));
        let distinct: std::collections::HashSet<usize> =
            (0..500u64).map(|s| sample_index(len, s)).collect();
        assert!(distinct.len() > 50, "index draw is not well distributed");
    }

    #[test]
    fn sample_index_empty_pool_is_zero() {
        assert_eq!(sample_index(0, 123), 0);
    }

    #[test]
    fn pool_verb_at_index_wraps() {
        let len = SPINNER_VERBS.len();
        assert_eq!(pool_verb_at_index(0), "Accomplishing");
        assert_eq!(pool_verb_at_index(len - 1), "Zigzagging");
        assert_eq!(pool_verb_at_index(len), "Accomplishing");
        assert_eq!(pool_verb_at_index(len + 6), "Beboppin'");
    }

    #[test]
    fn sample_verb_is_from_pool() {
        for _ in 0..100 {
            assert!(SPINNER_VERBS.contains(&sample_verb()));
        }
    }

    #[test]
    fn todo_leader_verb_prefers_active_form() {
        let todo = CurrentTodo {
            subject: "Build project".to_string(),
            active_form: Some("Compiling".to_string()),
        };
        assert_eq!(todo_leader_verb(Some(&todo)).as_deref(), Some("Compiling"));
    }

    #[test]
    fn todo_leader_verb_falls_back_to_subject() {
        let todo = CurrentTodo {
            subject: "Running tests".to_string(),
            active_form: None,
        };
        assert_eq!(
            todo_leader_verb(Some(&todo)).as_deref(),
            Some("Running tests")
        );
    }

    #[test]
    fn todo_leader_verb_skips_empty_active_form() {
        let todo = CurrentTodo {
            subject: "Running tests".to_string(),
            active_form: Some(String::new()),
        };
        assert_eq!(
            todo_leader_verb(Some(&todo)).as_deref(),
            Some("Running tests")
        );
    }

    #[test]
    fn todo_leader_verb_none_without_todo() {
        assert_eq!(todo_leader_verb(None), None);
        let empty = CurrentTodo {
            subject: String::new(),
            active_form: None,
        };
        assert_eq!(todo_leader_verb(Some(&empty)), None);
    }

    #[test]
    fn todo_leader_verb_used_verbatim_not_pool_matched() {
        let todo = CurrentTodo {
            subject: "Polishing the brass".to_string(),
            active_form: Some("Squeaking the wheels".to_string()),
        };
        let verb = todo_leader_verb(Some(&todo)).unwrap();
        assert_eq!(verb, "Squeaking the wheels");
        assert!(!SPINNER_VERBS.contains(&verb.as_str()));
    }
}
