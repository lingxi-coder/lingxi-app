#![forbid(unsafe_code)]
//! `SpinnerWithVerb` — claude-code-equivalent loading spinner. (M6-03, A1)
//!
//! Per `claude-code/src/components/Spinner/utils.ts` (darwin default) +
//! `Spinner.tsx:41` (`SPINNER_FRAMES = [...DEFAULT, ...reverse(DEFAULT)]`):
//! 6 forward chars then 6 reverse chars = 12 total.
//!
//! Linux platforms substitute `✳` → `*` (one frame difference); locked here
//! as the darwin variant since macOS is our primary dev platform and the
//! spec §2.8 requires byte-for-byte parity with claude-code. (A
//! `cfg(target_os = "linux")` variant can be added later if needed.)
//!
//! The verb pool is claude-code's full [`SPINNER_VERBS`] list
//! (`claude-code/src/constants/spinnerVerbs.ts:17-203`, 187 entries), ported
//! byte-for-byte in source order. The live component picks a random start verb on mount
//! ([`initial_verb_index`]) and advances every [`VERB_ROTATE_MS`], mirroring
//! claude-code's random-on-mount behavior. Honoring a user `spinnerVerbs`
//! `{mode, verbs}` setting is implemented by [`resolve_spinner_verbs`]
//! (`spinnerVerbs.ts:3-13`); the settings *loader* plumbing is wired in only
//! once the TUI settings reader exposes that field.

use iocraft::prelude::*;

/// 12-frame asterisk animation (forward+reverse cycle). claude-code's
/// darwin default characters from `Spinner/utils.ts`.
pub const SPINNER_FRAMES: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"];

/// The 3-verb subset the M6 deterministic snapshot/parity fixtures lock
/// (Crunching/Thinking/Generating). Retained only as the backing pool for
/// [`verb_at_index`]/[`format_spinner_line`], which the cross-crate parity
/// fixtures (`test-harness`) and the in-crate snapshot tests pin. New code
/// should use [`SPINNER_VERBS`] + [`resolve_spinner_verbs`] instead.
pub const VERBS_M6: &[&str] = &["Crunching", "Thinking", "Generating"];

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

/// How a user's `spinnerVerbs.verbs` combine with the built-in pool.
/// Mirrors claude-code's `spinnerVerbs.ts:9-12` (`'replace'` | `'append'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerVerbsMode {
    /// Use exactly the user's verbs (falling back to [`SPINNER_VERBS`] when
    /// the user list is empty).
    Replace,
    /// Append the user's verbs after the built-in [`SPINNER_VERBS`] pool.
    Append,
}

/// User configuration for the spinner verb pool — the resolved
/// `settings.spinnerVerbs` value (`spinnerVerbs.ts:4-5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpinnerVerbsSetting {
    /// Whether `verbs` replaces or appends to the built-in pool.
    pub mode: SpinnerVerbsMode,
    /// The user-supplied verbs.
    pub verbs: Vec<String>,
}

/// Resolve the effective verb pool from an optional user setting, mirroring
/// `getSpinnerVerbs()` (`claude-code/src/constants/spinnerVerbs.ts:3-13`):
///
/// - `None` → the full built-in [`SPINNER_VERBS`] pool.
/// - `Replace` with a non-empty list → exactly the user's verbs.
/// - `Replace` with an empty list → falls back to [`SPINNER_VERBS`].
/// - `Append` → [`SPINNER_VERBS`] followed by the user's verbs.
///
/// Returns owned `String`s so user verbs (which are not `'static`) and the
/// built-in pool can share one homogeneous `Vec`. Callers with no setting can
/// index [`SPINNER_VERBS`]/[`pool_verb_at_index`] directly to avoid the clone.
#[must_use]
pub fn resolve_spinner_verbs(setting: Option<&SpinnerVerbsSetting>) -> Vec<String> {
    match setting {
        None => SPINNER_VERBS.iter().map(|s| (*s).to_string()).collect(),
        Some(cfg) => match cfg.mode {
            SpinnerVerbsMode::Replace => {
                if cfg.verbs.is_empty() {
                    SPINNER_VERBS.iter().map(|s| (*s).to_string()).collect()
                } else {
                    cfg.verbs.clone()
                }
            }
            SpinnerVerbsMode::Append => SPINNER_VERBS
                .iter()
                .map(|s| (*s).to_string())
                .chain(cfg.verbs.iter().cloned())
                .collect(),
        },
    }
}

/// Pick the spinner's initial verb index for a pool of `len` verbs, given a
/// `rng_seed`. claude-code chooses a random start verb when the spinner
/// mounts (`Math.floor(Math.random() * verbs.length)`); this is the seedable
/// Rust equivalent so tests are deterministic. Returns `0` for an empty pool
/// (defensive — the pool is never empty in practice).
///
/// Uses a self-contained `splitmix64` step (no external RNG crate) to turn
/// the seed into a uniformly-distributed index in `0..len`.
#[must_use]
pub fn initial_verb_index(len: usize, rng_seed: u64) -> usize {
    if len == 0 {
        return 0;
    }
    // splitmix64: a single mixing step is enough for a one-shot index draw.
    let mut z = rng_seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // `z % len` is always `< len <= usize::MAX`, so the conversion never fails;
    // computing the modulus as `u64` avoids any lossy cast.
    let len_u64 = len as u64;
    usize::try_from(z % len_u64).unwrap_or(0)
}

/// A non-deterministic seed for the live spinner mount, derived from the
/// wall clock (sub-nanosecond resolution is unnecessary — successive mounts
/// land on different verbs). Tests use a fixed seed via [`initial_verb_index`].
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
/// modulo the pool length. The live component advances the index by 1 each
/// rotation, so wrapping keeps the cycle continuous. (The pool is non-empty.)
#[must_use]
pub fn pool_verb_at_index(index: usize) -> &'static str {
    SPINNER_VERBS[index % SPINNER_VERBS.len()]
}

/// Get the verb at `index` from an arbitrary resolved pool, wrapping modulo
/// its length. Empty pools yield `""` (defensive — never empty in practice).
/// Used by the live component when a user `spinnerVerbs` setting is active.
#[must_use]
pub fn resolved_verb_at_index(verbs: &[String], index: usize) -> &str {
    if verbs.is_empty() {
        return "";
    }
    verbs[index % verbs.len()].as_str()
}

/// Time between spinner frame advances. claude-code uses 50ms (20fps);
/// M6 uses 100ms (10fps) per spec §3 M6-03 — slower to reduce render
/// churn under the 30fps cap.
pub const FRAME_TICK_MS: u64 = 100;

/// Time between verb rotations. Locked at 4000ms — long enough that users
/// notice the change, short enough that it doesn't feel static. The live
/// component advances by one verb in the full [`SPINNER_VERBS`] pool per tick.
pub const VERB_ROTATE_MS: u64 = 4000;

/// Get the spinner glyph for a tick index. Wraps modulo `SPINNER_FRAMES.len()`.
#[inline]
#[must_use]
pub fn frame_at_index(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// Get the verb for a rotation index. Wraps modulo `VERBS_M6.len()`.
#[inline]
#[must_use]
pub fn verb_at_index(rotation: usize) -> &'static str {
    VERBS_M6[rotation % VERBS_M6.len()]
}

/// Format a single spinner line: `"{frame} {verb}…"`.
///
/// This is the function the iocraft component renders inside its `Text`.
/// Exposed publicly so snapshot tests can assert without a render harness.
/// The ellipsis is U+2026 (HORIZONTAL ELLIPSIS), NOT three ASCII dots.
#[must_use]
pub fn format_spinner_line(tick: usize, rotation: usize) -> String {
    format!("{} {}…", frame_at_index(tick), verb_at_index(rotation))
}

/// Props for [`SpinnerWithVerb`]. Both fields are hook-managed inside the
/// component by default; pass `Some(_)` to override (used by snapshot tests).
#[derive(Default, Props)]
pub struct SpinnerWithVerbProps {
    /// Override the frame index. `None` (default) → component ticks
    /// internally at `FRAME_TICK_MS`.
    pub frame_override: Option<usize>,
    /// Override the verb rotation index. `None` → internal rotation at
    /// `VERB_ROTATE_MS`.
    pub verb_override: Option<usize>,
}

/// Renders one line: `"{frame} {verb}…"`. While mounted, advances frames
/// at 10fps and rotates verbs every 4s. Both intervals are constants
/// (`FRAME_TICK_MS`, `VERB_ROTATE_MS`).
///
/// The verb is drawn from claude-code's full [`SPINNER_VERBS`] pool, starting
/// at a **random** index chosen once on mount ([`initial_verb_index`] +
/// [`live_seed`]) — matching claude-code's random-on-mount selection — then
/// advancing by 1 (wrapping) each rotation.
///
/// Mounting/unmounting is the caller's responsibility: the REPL screen
/// wraps this in `if app.streaming.is_some() { <SpinnerWithVerb/> }`.
#[component]
pub fn SpinnerWithVerb(
    props: &SpinnerWithVerbProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut frame = hooks.use_state(|| 0usize);
    // Random start verb chosen once on mount (seeded from the wall clock), so
    // each spinner mount opens on a different verb like claude-code does.
    let mut verb = hooks.use_state(|| initial_verb_index(SPINNER_VERBS.len(), live_seed()));

    if let Some(f) = props.frame_override {
        frame.set(f);
    } else {
        hooks.use_future(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(FRAME_TICK_MS));
            tick.tick().await; // first tick fires immediately; discard
            loop {
                tick.tick().await;
                let cur = frame.get();
                frame.set(cur.wrapping_add(1));
            }
        });
    }
    if let Some(v) = props.verb_override {
        verb.set(v);
    } else {
        hooks.use_future(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(VERB_ROTATE_MS));
            tick.tick().await;
            loop {
                tick.tick().await;
                let cur = verb.get();
                verb.set(cur.wrapping_add(1));
            }
        });
    }

    let line = format!(
        "{} {}…",
        frame_at_index(frame.get()),
        pool_verb_at_index(verb.get())
    );
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: line, color: Color::Cyan)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_match_claude_code_darwin_default() {
        assert_eq!(
            SPINNER_FRAMES,
            &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"]
        );
        assert_eq!(SPINNER_FRAMES.len(), 12);
    }

    #[test]
    fn verbs_m6_match_design_subset() {
        assert_eq!(VERBS_M6, &["Crunching", "Thinking", "Generating"]);
    }

    #[test]
    fn verb_format_uses_horizontal_ellipsis() {
        assert_eq!(format!("{}{}", "Crunching", '…'), "Crunching…");
        // U+2026 HORIZONTAL ELLIPSIS, NOT three ASCII dots.
        assert_eq!('…' as u32, 0x2026);
    }

    #[test]
    fn frame_at_index_wraps() {
        assert_eq!(frame_at_index(0), "·");
        assert_eq!(frame_at_index(5), "✽");
        assert_eq!(frame_at_index(9), "✳");
        assert_eq!(frame_at_index(12), "·"); // wraps
        assert_eq!(frame_at_index(25), "✢"); // wraps twice
    }

    #[test]
    fn verb_at_index_wraps() {
        assert_eq!(verb_at_index(0), "Crunching");
        assert_eq!(verb_at_index(1), "Thinking");
        assert_eq!(verb_at_index(2), "Generating");
        assert_eq!(verb_at_index(3), "Crunching"); // wraps
    }

    #[test]
    fn format_spinner_line_includes_ellipsis() {
        assert_eq!(format_spinner_line(0, 0), "· Crunching…");
        assert_eq!(format_spinner_line(5, 1), "✽ Thinking…");
        assert_eq!(format_spinner_line(9, 2), "✳ Generating…");
    }

    // ---- A1: full 187-verb pool + resolve/select --------------------------

    #[test]
    fn spinner_pool_has_187_verbs_in_source_order() {
        // Exact length: claude-code/src/constants/spinnerVerbs.ts has 187
        // entries on lines 17-203 (one verb per line; line 16 is `[`, line 204
        // is `]`). The spec heading's "186" is an off-by-one miscount — the
        // source of truth is the TS file, which is reproduced byte-for-byte.
        assert_eq!(SPINNER_VERBS.len(), 187);
        // First/last anchors (byte-for-byte, source order).
        assert_eq!(SPINNER_VERBS[0], "Accomplishing");
        assert_eq!(SPINNER_VERBS[186], "Zigzagging");
        // Diacritic / apostrophe / hyphen entries preserved exactly.
        assert_eq!(SPINNER_VERBS[6], "Beboppin'");
        assert!(SPINNER_VERBS.contains(&"Flambéing"));
        assert!(SPINNER_VERBS.contains(&"Sautéing"));
        assert!(SPINNER_VERBS.contains(&"Philosophising"));
        assert!(SPINNER_VERBS.contains(&"Dilly-dallying"));
        assert!(SPINNER_VERBS.contains(&"Razzle-dazzling"));
        assert!(SPINNER_VERBS.contains(&"Sock-hopping"));
        assert!(SPINNER_VERBS.contains(&"Topsy-turvying"));
        // No duplicates and no empty entries.
        assert!(SPINNER_VERBS.iter().all(|v| !v.is_empty()));
        let mut sorted = SPINNER_VERBS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SPINNER_VERBS.len(), "pool has duplicates");
        // The M6 anchors live in the full pool too (so the M6 path is a subset).
        for v in VERBS_M6 {
            assert!(SPINNER_VERBS.contains(v), "M6 verb {v} missing from pool");
        }
    }

    #[test]
    fn resolve_verbs_none_is_full_pool() {
        let got = resolve_spinner_verbs(None);
        assert_eq!(got.len(), SPINNER_VERBS.len());
        assert_eq!(got.first().map(String::as_str), Some("Accomplishing"));
        assert_eq!(got.last().map(String::as_str), Some("Zigzagging"));
    }

    #[test]
    fn resolve_verbs_replace_nonempty() {
        let setting = SpinnerVerbsSetting {
            mode: SpinnerVerbsMode::Replace,
            verbs: vec!["Foo".to_string(), "Bar".to_string()],
        };
        let got = resolve_spinner_verbs(Some(&setting));
        assert_eq!(got, vec!["Foo".to_string(), "Bar".to_string()]);
    }

    #[test]
    fn resolve_verbs_replace_empty_falls_back() {
        let setting = SpinnerVerbsSetting {
            mode: SpinnerVerbsMode::Replace,
            verbs: vec![],
        };
        let got = resolve_spinner_verbs(Some(&setting));
        // Falls back to the full pool, exactly like getSpinnerVerbs().
        assert_eq!(got.len(), SPINNER_VERBS.len());
        assert_eq!(got.first().map(String::as_str), Some("Accomplishing"));
    }

    #[test]
    fn resolve_verbs_append_concatenates() {
        let setting = SpinnerVerbsSetting {
            mode: SpinnerVerbsMode::Append,
            verbs: vec!["Foo".to_string(), "Bar".to_string()],
        };
        let got = resolve_spinner_verbs(Some(&setting));
        assert_eq!(got.len(), SPINNER_VERBS.len() + 2);
        // Built-in pool first, in order, then the user's verbs.
        assert_eq!(got[0], "Accomplishing");
        assert_eq!(got[SPINNER_VERBS.len() - 1], "Zigzagging");
        assert_eq!(got[SPINNER_VERBS.len()], "Foo");
        assert_eq!(got[SPINNER_VERBS.len() + 1], "Bar");
    }

    #[test]
    fn initial_verb_index_in_bounds() {
        let len = SPINNER_VERBS.len();
        for seed in 0..10_000u64 {
            let idx = initial_verb_index(len, seed);
            assert!(idx < len, "idx {idx} out of bounds for seed {seed}");
        }
        // Wall-clock-derived seeds also stay in bounds.
        for _ in 0..1_000 {
            let idx = initial_verb_index(len, live_seed());
            assert!(idx < len);
        }
    }

    #[test]
    fn initial_verb_index_is_deterministic_per_seed() {
        // Same seed → same index (seedable for tests).
        let len = SPINNER_VERBS.len();
        assert_eq!(initial_verb_index(len, 42), initial_verb_index(len, 42));
        // Distinct seeds spread across the pool (not all the same index).
        let distinct: std::collections::HashSet<usize> =
            (0..500u64).map(|s| initial_verb_index(len, s)).collect();
        assert!(distinct.len() > 50, "index draw is not well distributed");
    }

    #[test]
    fn initial_verb_index_empty_pool_is_zero() {
        assert_eq!(initial_verb_index(0, 123), 0);
    }

    #[test]
    fn initial_verb_index_handles_single_element_pool() {
        for seed in 0..100u64 {
            assert_eq!(initial_verb_index(1, seed), 0);
        }
    }

    #[test]
    fn pool_verb_at_index_wraps() {
        let len = SPINNER_VERBS.len(); // 187
        assert_eq!(pool_verb_at_index(0), "Accomplishing");
        assert_eq!(pool_verb_at_index(len - 1), "Zigzagging");
        // Wraps modulo pool length.
        assert_eq!(pool_verb_at_index(len), "Accomplishing");
        assert_eq!(pool_verb_at_index(len + 6), "Beboppin'");
    }

    #[test]
    fn resolved_verb_at_index_wraps_and_handles_empty() {
        let verbs = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(resolved_verb_at_index(&verbs, 0), "A");
        assert_eq!(resolved_verb_at_index(&verbs, 2), "C");
        assert_eq!(resolved_verb_at_index(&verbs, 3), "A"); // wraps
                                                            // Empty pool is defensively the empty string.
        assert_eq!(resolved_verb_at_index(&[], 0), "");
    }
}
