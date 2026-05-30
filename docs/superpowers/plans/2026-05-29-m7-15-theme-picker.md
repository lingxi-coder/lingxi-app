# M7-15 — Theme Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand `lingxi-tui`'s M6 fixed palette (`TuiTheme` — 4 const colors) into a `Theme` struct + a registry of claude-code's 6 named themes (dark / light / dark-daltonized / light-daltonized / dark-ansi / light-ansi), store the active choice in `AppState.theme`, ship a `/theme` picker (a `Screen` reached via `active_screen`, matching claude-code's `ThemePicker.tsx`) that arrow-selects + previews + applies a theme with live re-render, make `render::syntax`'s `.tmTheme` selection follow `AppState.theme`, and centralize the `// TODO(M7-15): theme constant` colors that M7-04/05 hardcoded into the `Theme` struct.

**Architecture:** `theme.rs` gains a `Theme` value-struct (one `iocraft::Color` field per claude-code `Theme` key the TUI actually consumes), a `ThemeName` enum (the 6 names), a `ThemeSetting` enum (`auto` + the 6 names — mirrors claude-code's wire values), a `theme_registry()` lookup `ThemeName -> Theme` carrying the exact RGB/ANSI colors from `claude-code/src/utils/theme.ts`, and `auto`-resolution (default to `dark` headless — no terminal-background probe in M7). The old `TuiTheme` associated consts are kept as a thin shim mapping to `Theme::dark()` so M6 call sites compile unchanged, then migrated. `AppState` gains `theme: Theme` (the resolved active palette) + `theme_setting: ThemeSetting` (the stored preference). The picker is `screens/theme.rs` — a new `Screen::Theme` variant on the M7-11 `active_screen` enum (priority 2 in the §2.5 dispatcher); it lists the 6 themes (+`auto`), tracks a `highlighted` index for live preview, and on `Enter` commits `theme_setting` → resolves `theme` → best-effort persists to `~/.claude/settings.json` via the existing config write path. `render::syntax` (M7-02) gains a `tm_theme_for(theme_name: ThemeName) -> &syntect::highlighting::Theme` that swaps the bundled `.tmTheme` by active theme name; the highlight entrypoint takes the active `ThemeName`. Renderers/StatusLine/diff read colors from the passed `&Theme` instead of `TuiTheme` consts.

**Tech Stack:** Rust 1.82 (pinned via `lingxi-code/rust-toolchain.toml`), iocraft `=0.8.3` (`View` not `Box`; `Color::Rgb { r, g, b }` for truecolor, named `Color` variants for ANSI), `syntect` (M7-02; `ThemeSet`, lazy `.tmTheme` load), `insta = "1.40"` snapshots, `serde_json` (settings read-modify-write). claude-code references: `src/components/ThemePicker.tsx`, `src/utils/theme.ts`, `src/commands/theme/theme.tsx`.

---

## Prerequisites & Dependencies

Run cargo **from inside `lingxi-code/`** (rust-toolchain pins 1.82; repo-root runs use the host toolchain → spurious lint noise — this bit M6-08). Real crate path: `lingxi-code/crates/tui/`.

- **M7-02 (syntect + StructuredDiff) MUST be merged first.** This plan makes the syntect `.tmTheme` selection a function of `AppState.theme`. **Verify before starting:** `ls lingxi-code/crates/tui/src/render/syntax.rs` exists. Grep for the real highlight entrypoint + current theme handling: `grep -n "ThemeSet\|tm_theme\|fn highlight\|\.tmTheme\|highlighting::Theme\|load_defaults" lingxi-code/crates/tui/src/render/syntax.rs`. **Do NOT invent names** — Task 6 adapts whatever M7-02 named (`highlight`, `highlight_code`, `SyntaxHighlighter`, a `theme: &str` arg, etc.). If M7-02 hardcoded a single theme name (e.g. `"base16-ocean.dark"`), Task 6 replaces that constant with a `ThemeName -> &str` map.
- **M7-11 (Doctor screen / `active_screen` infra) MUST be merged first** if the picker is a screen. **Verify:** `grep -rn "active_screen\|enum Screen\|pub enum Screen" lingxi-code/crates/tui/src/` returns the `Screen` enum + `AppState.active_screen` + the §2.5 dispatcher branch (priority 2). **Do NOT invent the enum name or its variant convention** — Task 4 adds a `Theme` variant to whatever M7-11 defined and routes it through the existing dispatcher and the existing `Esc`/`q` return path. If M7-11 is NOT yet merged when this plan executes, fall back to the documented escape hatch in Task 4 (a self-contained `pending_theme_picker: Option<ThemePickerState>` slot on `AppState`, routed at a new priority-2 branch in `handle_live_key`, mirroring M6's `pending_permission` focus-trap shape) and leave a `// TODO: fold into Screen::Theme once M7-11 active_screen lands` note. Either way the picker is reached via the `/theme` command (claude-code parity) — see Task 5.
- **M7-04 / M7-05 (message renderers) leave `// TODO(M7-15): theme constant` markers** where they hardcoded iocraft colors for claude-code's `warning` / `success` / `planMode` (and any other) semantic colors instead of theme colors (confirmed in the M7-04 plan's color-mapping table: warning→`Color::Yellow`, success→`Color::Green`, planMode→`Color::Magenta`, each with a `// TODO(M7-15): theme constant` comment). Task 7 greps for these and resolves them. If M7-04/05 are NOT yet merged when this plan executes, Task 7 is a no-op except for the StatusLine/assistant/user-text/diff call sites that DO exist — document the deferral inline and the M7-16 final review re-runs the grep.
- **`StyledLine` / diff color types (M7-01/02):** if Task 6/7 needs the markdown/diff styled-line type, grep first — `grep -rn "struct StyledLine\|pub struct Styled\|diffAdded\|diff_added" lingxi-code/crates/tui/src/render/`. Use the real name.
- **Telemetry:** baseline is **326 events**. **M7-15 adds 0 events.** No `ALL_EVENT_NAMES` changes. Do NOT register any telemetry name. (A `tengu_tui_screen_opened`-class event for the picker is deferred to M7-16's audit, per spec §2.7.)

## Literal-Lock Reference (read each source before coding)

Source root: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`.

- **`utils/theme.ts`** — the canonical color tables. `THEME_NAMES = ['dark','light','light-daltonized','dark-daltonized','light-ansi','dark-ansi']`; `THEME_SETTINGS = ['auto', ...THEME_NAMES]`. Each named theme is a `Theme` object of `rgb(r,g,b)` strings (RGB themes) or `ansi:<name>` strings (the two `-ansi` themes). The TUI consumes a **subset** of the ~80 keys; this plan ports exactly the keys the lingxi TUI renders (enumerated in Task 1). Copy the RGB triples / ANSI names **byte-for-byte** from `theme.ts` for those keys.
- **`components/ThemePicker.tsx`** — the picker UI. Locked option labels (exact, in this order — `auto` first only when the `AUTO_THEME` feature is on; lingxi ships `auto` unconditionally as the first option per spec §2.4):
  - `"Auto (match terminal)"` → value `auto`
  - `"Dark mode"` → value `dark`
  - `"Light mode"` → value `light`
  - `"Dark mode (colorblind-friendly)"` → value `dark-daltonized`
  - `"Light mode (colorblind-friendly)"` → value `light-daltonized`
  - `"Dark mode (ANSI colors only)"` → value `dark-ansi`
  - `"Light mode (ANSI colors only)"` → value `light-ansi`
  - Header (non-onboarding): `Theme` (bold, `permission` color). Sub-header: `Choose the text style that looks best with your terminal` (bold). Preview is a `StructuredDiff` over the fixed snippet `function greet() { -console.log("Hello, World!"); +console.log("Hello, Claude!"); }` — lingxi reuses its M7-02 `render::diff` for the preview.
- **`commands/theme/theme.tsx`** — the `/theme` command entry. Locked command name `theme`; it opens the ThemePicker. lingxi already registers a `theme` config field; this plan wires the `/theme` command to open `Screen::Theme`.

**Color mapping** (claude-code `Theme` key → lingxi `Theme` field → iocraft `Color`): RGB string `rgb(r,g,b)` → `Color::Rgb { r, g, b }`; ANSI string `ansi:red` → `Color::DarkRed`, `ansi:redBright` → `Color::Red`, `ansi:green`→`Color::DarkGreen`, `ansi:greenBright`→`Color::Green`, `ansi:yellow`→`Color::DarkYellow`, `ansi:yellowBright`→`Color::Yellow`, `ansi:blue`→`Color::DarkBlue`, `ansi:blueBright`→`Color::Blue`, `ansi:magenta`→`Color::DarkMagenta`, `ansi:magentaBright`→`Color::Magenta`, `ansi:cyan`→`Color::DarkCyan`, `ansi:cyanBright`→`Color::Cyan`, `ansi:white`→`Color::Grey`, `ansi:whiteBright`→`Color::White`, `ansi:black`→`Color::Black`, `ansi:blackBright`→`Color::DarkGrey`. (crossterm/iocraft's "bright" names drop the `Dark` prefix; the non-bright ANSI maps to the `Dark*` variant. This table is locked in Task 1 as a `const fn ansi(name) -> Color` helper with a unit test.)

---

## File Structure

**Modify:**
- `lingxi-code/crates/tui/src/theme.rs` — the heart of this plan. Add `ThemeName` enum, `ThemeSetting` enum, `Theme` struct (the per-render-key palette), `theme_registry()` / `Theme::dark()`/`light()`/etc. constructors carrying the locked colors, `ThemeSetting::resolve() -> ThemeName` (`auto`→`dark` headless), wire-string parse/format (`as_wire`/`from_wire`). Keep `struct TuiTheme` with its `ASSISTANT`/`USER`/`ERROR`/`DIM` consts as a **shim** delegating to `Theme::dark()` so M6 call sites compile, with a `// shim — migrated to Theme in M7-15` note.
- `lingxi-code/crates/tui/src/state.rs` — add `pub theme: Theme` + `pub theme_setting: ThemeSetting` to `AppState`; init in `new()` (default `ThemeSetting::Auto` → `Theme::dark()`) and `default_for_tests()`; add `set_theme(&mut self, setting: ThemeSetting)` that resolves + stores both fields.
- `lingxi-code/crates/tui/src/render/syntax.rs` (M7-02) — make the `.tmTheme` selection a function of `ThemeName` (Task 6). Add `tm_theme_for(name: ThemeName) -> &'static syntect::highlighting::Theme` (lazy via `once_cell`/`LazyLock`; §4 R9 lazy-load) + thread the active `ThemeName` into the highlight entrypoint.
- `lingxi-code/crates/tui/src/components/status_line.rs` — `StatusLine` reads label/segment colors from a passed `&Theme` (or `Theme` prop) instead of bare/`TuiTheme` colors (Task 8).
- `lingxi-code/crates/tui/src/components/messages/*.rs` + `scrollback.rs` + `app.rs::render_screen` — thread the active `&Theme` to renderers; resolve `// TODO(M7-15): theme constant` sites (Task 7).
- `lingxi-code/crates/tui/src/screens/mod.rs` — `pub mod theme;`.
- `lingxi-code/crates/tui/src/root.rs` — route `Screen::Theme` key handling through the existing `active_screen` priority-2 branch (Task 4); no parallel key path (§2.5).
- The `/theme` command site (Task 5) — grep for where slash commands open screens (`grep -rn "Screen::\|active_screen =\|\"/theme\"\|\"theme\"" lingxi-code/crates/tui/src/`); wire `/theme` → open `Screen::Theme`.

**Create:**
- `lingxi-code/crates/tui/src/screens/theme.rs` — `ThemePickerState { options: Vec<ThemeSetting>, highlighted: usize }` + pure key-handler `theme_picker_handle_key(state, &mut AppState, key) -> ThemePickerOutcome` (Up/Down move `highlighted` → live-preview `AppState.theme`; Enter commit + close; Esc cancel → restore prior setting + close) + a `#[component] ThemePickerScreen` + pure `render_theme_picker_to_string(&ThemePickerState, &Theme) -> String` (snapshot oracle: header + option rows with the locked labels + a `❯ ` pointer on `highlighted` + the diff-preview lines).

**Create (test files):** `lingxi-code/crates/tui/tests/`
- `theme_registry.rs` — registry/color/wire-roundtrip + `auto` resolution unit assertions.
- `theme_picker_behavior.rs` — picker list/highlight/preview/commit/cancel behavior.
- `theme_snapshots.rs` — insta snapshots of StatusLine + an assistant message + a code block under `dark` and `light`.
- `theme_syntax_follows.rs` — syntect `.tmTheme` differs between two themes for the same code.

**Snapshots land in:** `lingxi-code/crates/tui/tests/snapshots/` (insta auto-creates `*.snap`).

---

## Conventions (apply throughout)

- **iocraft:** `View` (not `Box`); truecolor `Color::Rgb { r, g, b }`; named `Color::*` for ANSI themes. Pattern: see `components/messages/assistant_text.rs` (cleanest) and `components/status_line.rs`.
- **Pure-fn-first / two-function pattern:** every renderable gets a `render_*_to_string(...) -> String` snapshot oracle plus a `#[component]` wrapper, matching the M6-04 / M7-04 house style.
- **`Theme` is a plain value struct** (`#[derive(Debug, Clone, Copy, PartialEq, Eq)]`) — all fields `iocraft::Color` (which is `Copy`). No iocraft `Props` derive on `Theme` itself; pass it by value/ref into components.
- **No new persistence logic** (§4 R7): theme persists through the **existing** `~/.claude/settings.json` `theme` field (already an allowlisted config field — `crates/tools/src/builtin/config.rs::CONFIG_FIELD_THEME`; `SettingsJson` is camelCase on the wire). Write = read-modify-write the JSON object with `theme: <wire string>` (Task 9 reuses the config-tool serialization shape — pretty JSON, trailing newline). If the write path isn't trivially reachable from the TUI crate without a new dep, the persistence is **best-effort**: log + continue, theme still applies for the session. Read at startup (Task 9) via the existing `Settings::load`/`EffectiveSettings` if reachable, else session-default `auto`.
- **Run cargo from inside `lingxi-code/`.**
- **Commit format:** `plan(M7-15 TN): <subject>` with trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. **Do not push. No tag push, amend, or `--no-verify`.**

---

## Task 1: `Theme` struct + `ThemeName`/`ThemeSetting` + ANSI map

**Files:**
- Modify: `lingxi-code/crates/tui/src/theme.rs`
- Test: `lingxi-code/crates/tui/src/theme.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test** — append to the `tests` mod in `theme.rs`:

```rust
#[test]
fn theme_names_count_and_wire_roundtrip() {
    use super::{ThemeName, ThemeSetting};
    // 6 renderable themes.
    assert_eq!(ThemeName::ALL.len(), 6);
    // Wire roundtrip for every setting (auto + 6 names).
    for s in ThemeSetting::ALL {
        assert_eq!(ThemeSetting::from_wire(s.as_wire()), Some(*s));
    }
    // Exact wire strings (claude-code parity).
    assert_eq!(ThemeSetting::Auto.as_wire(), "auto");
    assert_eq!(ThemeName::Dark.as_wire(), "dark");
    assert_eq!(ThemeName::DarkDaltonized.as_wire(), "dark-daltonized");
    assert_eq!(ThemeName::LightAnsi.as_wire(), "light-ansi");
    assert_eq!(ThemeSetting::from_wire("bogus"), None);
}

#[test]
fn ansi_map_covers_bright_and_dark() {
    use super::ansi;
    use iocraft::Color;
    assert!(matches!(ansi("ansi:red"), Color::DarkRed));
    assert!(matches!(ansi("ansi:redBright"), Color::Red));
    assert!(matches!(ansi("ansi:white"), Color::Grey));
    assert!(matches!(ansi("ansi:whiteBright"), Color::White));
    assert!(matches!(ansi("ansi:black"), Color::Black));
    assert!(matches!(ansi("ansi:blackBright"), Color::DarkGrey));
}

#[test]
fn auto_resolves_to_dark_headless() {
    use super::{ThemeName, ThemeSetting};
    assert_eq!(ThemeSetting::Auto.resolve(), ThemeName::Dark);
    assert_eq!(ThemeSetting::Named(ThemeName::Light).resolve(), ThemeName::Light);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib theme::tests::theme_names_count_and_wire_roundtrip`
Expected: FAIL — `ThemeName`, `ThemeSetting`, `ansi` not defined.

- [ ] **Step 3: Write minimal implementation** — add to `theme.rs` (above the existing `TuiTheme`):

```rust
/// One of claude-code's 6 renderable themes (`utils/theme.ts` `THEME_NAMES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Dark,
    Light,
    LightDaltonized,
    DarkDaltonized,
    LightAnsi,
    DarkAnsi,
}

impl ThemeName {
    /// All renderable theme names, in `THEME_NAMES` order.
    pub const ALL: [ThemeName; 6] = [
        ThemeName::Dark,
        ThemeName::Light,
        ThemeName::LightDaltonized,
        ThemeName::DarkDaltonized,
        ThemeName::LightAnsi,
        ThemeName::DarkAnsi,
    ];

    /// Wire string, byte-for-byte with claude-code's `ThemeName`.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
            ThemeName::LightDaltonized => "light-daltonized",
            ThemeName::DarkDaltonized => "dark-daltonized",
            ThemeName::LightAnsi => "light-ansi",
            ThemeName::DarkAnsi => "dark-ansi",
        }
    }

    #[must_use]
    pub fn from_wire(s: &str) -> Option<ThemeName> {
        ThemeName::ALL.into_iter().find(|n| n.as_wire() == s)
    }
}

/// A theme *preference* as stored in config. `Auto` follows the terminal and
/// resolves to a [`ThemeName`] at runtime (claude-code `ThemeSetting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSetting {
    Auto,
    Named(ThemeName),
}

impl ThemeSetting {
    /// `auto` + the 6 names, in `THEME_SETTINGS` order.
    pub const ALL: [ThemeSetting; 7] = [
        ThemeSetting::Auto,
        ThemeSetting::Named(ThemeName::Dark),
        ThemeSetting::Named(ThemeName::Light),
        ThemeSetting::Named(ThemeName::LightDaltonized),
        ThemeSetting::Named(ThemeName::DarkDaltonized),
        ThemeSetting::Named(ThemeName::LightAnsi),
        ThemeSetting::Named(ThemeName::DarkAnsi),
    ];

    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            ThemeSetting::Auto => "auto",
            ThemeSetting::Named(n) => n.as_wire(),
        }
    }

    #[must_use]
    pub fn from_wire(s: &str) -> Option<ThemeSetting> {
        if s == "auto" {
            return Some(ThemeSetting::Auto);
        }
        ThemeName::from_wire(s).map(ThemeSetting::Named)
    }

    /// Resolve to a concrete renderable theme. Headless TUI cannot probe the
    /// terminal background, so `Auto` resolves to `Dark` (claude-code's
    /// dark-default fallback). Real terminal-bg detection is deferred to M8.
    #[must_use]
    pub const fn resolve(self) -> ThemeName {
        match self {
            ThemeSetting::Auto => ThemeName::Dark,
            ThemeSetting::Named(n) => n,
        }
    }
}

/// Map a claude-code `ansi:<name>` color string to an iocraft `Color`.
/// "Bright" → the un-prefixed crossterm variant; non-bright → the `Dark*`
/// variant. Used only by the two `-ansi` themes.
#[must_use]
pub fn ansi(name: &str) -> iocraft::Color {
    use iocraft::Color;
    match name {
        "ansi:black" => Color::Black,
        "ansi:blackBright" => Color::DarkGrey,
        "ansi:red" => Color::DarkRed,
        "ansi:redBright" => Color::Red,
        "ansi:green" => Color::DarkGreen,
        "ansi:greenBright" => Color::Green,
        "ansi:yellow" => Color::DarkYellow,
        "ansi:yellowBright" => Color::Yellow,
        "ansi:blue" => Color::DarkBlue,
        "ansi:blueBright" => Color::Blue,
        "ansi:magenta" => Color::DarkMagenta,
        "ansi:magentaBright" => Color::Magenta,
        "ansi:cyan" => Color::DarkCyan,
        "ansi:cyanBright" => Color::Cyan,
        "ansi:white" => Color::Grey,
        "ansi:whiteBright" => Color::White,
        _ => Color::Reset,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib theme::tests`
Expected: PASS (the 3 new tests + the existing 4 `TuiTheme` tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/theme.rs
git commit -m "plan(M7-15 T1): add ThemeName/ThemeSetting enums + ansi color map

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: `Theme` struct + registry (locked colors for all 6 themes)

The `Theme` struct carries exactly the keys the lingxi TUI renders. Port these from `claude-code/src/utils/theme.ts` (read it first): `text`, `inactive` (→ dim), `error`, `success`, `warning`, `permission`, `plan_mode` (`planMode`), `suggestion`, `diff_added` (`diffAdded`), `diff_removed` (`diffRemoved`), `diff_added_word` (`diffAddedWord`), `diff_removed_word` (`diffRemovedWord`), `claude` (assistant accent). The assistant body color (M6 cyan) maps to `claude` going forward.

**Files:**
- Modify: `lingxi-code/crates/tui/src/theme.rs`
- Test: `lingxi-code/crates/tui/src/theme.rs` (inline tests)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn registry_returns_distinct_palettes_with_locked_colors() {
    use super::{theme_for, ThemeName};
    use iocraft::Color;
    let dark = theme_for(ThemeName::Dark);
    let light = theme_for(ThemeName::Light);
    // dark.text = rgb(255,255,255); light.text = rgb(0,0,0)  (theme.ts).
    assert_eq!(dark.text, Color::Rgb { r: 255, g: 255, b: 255 });
    assert_eq!(light.text, Color::Rgb { r: 0, g: 0, b: 0 });
    // dark.error = rgb(255,107,128); light.error = rgb(171,43,63).
    assert_eq!(dark.error, Color::Rgb { r: 255, g: 107, b: 128 });
    assert_eq!(light.error, Color::Rgb { r: 171, g: 43, b: 63 });
    // dark.claude = rgb(215,119,87)  (the Claude orange accent).
    assert_eq!(dark.claude, Color::Rgb { r: 215, g: 119, b: 87 });
    // ANSI theme uses named colors, not Rgb.
    let dark_ansi = theme_for(ThemeName::DarkAnsi);
    assert_eq!(dark_ansi.error, Color::Red); // ansi:redBright
    assert_eq!(dark_ansi.success, Color::Green); // ansi:greenBright
    // Every theme resolves (no panic) and is Copy.
    for n in ThemeName::ALL {
        let _t: super::Theme = theme_for(n);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib theme::tests::registry_returns_distinct_palettes_with_locked_colors`
Expected: FAIL — `Theme`, `theme_for` not defined.

- [ ] **Step 3: Write minimal implementation** — add to `theme.rs`. Use a `rgb` helper + the `ansi` helper from Task 1. **Copy every triple from `theme.ts` byte-for-byte** (the test pins a few; the executor fills the rest from the file):

```rust
use iocraft::Color;

/// `Color::Rgb` shorthand.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

/// The active render palette. One iocraft `Color` per claude-code `Theme`
/// key the lingxi TUI consumes. All RGB/ANSI values copied from
/// `claude-code/src/utils/theme.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Default body text (`text`).
    pub text: Color,
    /// Dim/inactive (`inactive`) — system hints, footnotes.
    pub dim: Color,
    /// Error (`error`).
    pub error: Color,
    /// Success / approved (`success`).
    pub success: Color,
    /// Warning (`warning`).
    pub warning: Color,
    /// Permission accent (`permission`) — picker header, dialogs.
    pub permission: Color,
    /// Plan-mode accent (`planMode`).
    pub plan_mode: Color,
    /// Suggestion / completion accent (`suggestion`).
    pub suggestion: Color,
    /// Claude/assistant accent (`claude`) — assistant body color.
    pub claude: Color,
    /// Diff added line bg/fg (`diffAdded`).
    pub diff_added: Color,
    /// Diff removed line (`diffRemoved`).
    pub diff_removed: Color,
    /// Diff added word-level (`diffAddedWord`).
    pub diff_added_word: Color,
    /// Diff removed word-level (`diffRemovedWord`).
    pub diff_removed_word: Color,
}

impl Theme {
    /// Dark theme — `theme.ts` `darkTheme`.
    #[must_use]
    pub const fn dark() -> Theme {
        Theme {
            text: rgb(255, 255, 255),
            dim: rgb(153, 153, 153),       // inactive
            error: rgb(255, 107, 128),
            success: rgb(78, 186, 101),
            warning: rgb(255, 193, 7),
            permission: rgb(177, 185, 249),
            plan_mode: rgb(72, 150, 140),
            suggestion: rgb(177, 185, 249),
            claude: rgb(215, 119, 87),
            diff_added: rgb(34, 92, 43),
            diff_removed: rgb(122, 41, 54),
            diff_added_word: rgb(56, 166, 96),
            diff_removed_word: rgb(179, 89, 107),
        }
    }

    /// Light theme — `theme.ts` `lightTheme`.
    #[must_use]
    pub const fn light() -> Theme {
        Theme {
            text: rgb(0, 0, 0),
            dim: rgb(102, 102, 102),       // inactive
            error: rgb(171, 43, 63),
            success: rgb(44, 122, 57),
            warning: rgb(150, 108, 30),
            permission: rgb(87, 105, 247),
            plan_mode: rgb(0, 102, 102),
            suggestion: rgb(87, 105, 247),
            claude: rgb(215, 119, 87),
            diff_added: rgb(105, 219, 124),
            diff_removed: rgb(255, 168, 180),
            diff_added_word: rgb(47, 157, 68),
            diff_removed_word: rgb(209, 69, 75),
        }
    }

    /// Dark daltonized — `theme.ts` `darkDaltonizedTheme`.
    #[must_use]
    pub const fn dark_daltonized() -> Theme {
        Theme {
            text: rgb(255, 255, 255),
            dim: rgb(153, 153, 153),
            error: rgb(255, 102, 102),
            success: rgb(51, 153, 255),    // blue-for-green
            warning: rgb(255, 204, 0),
            permission: rgb(153, 204, 255),
            plan_mode: rgb(102, 153, 153),
            suggestion: rgb(153, 204, 255),
            claude: rgb(255, 153, 51),
            diff_added: rgb(0, 68, 102),
            diff_removed: rgb(102, 0, 0),
            diff_added_word: rgb(0, 119, 179),
            diff_removed_word: rgb(179, 0, 0),
        }
    }

    /// Light daltonized — `theme.ts` `lightDaltonizedTheme`.
    #[must_use]
    pub const fn light_daltonized() -> Theme {
        Theme {
            text: rgb(0, 0, 0),
            dim: rgb(102, 102, 102),
            error: rgb(204, 0, 0),
            success: rgb(0, 102, 153),     // blue-for-green
            warning: rgb(255, 153, 0),
            permission: rgb(51, 102, 255),
            plan_mode: rgb(51, 102, 102),
            suggestion: rgb(51, 102, 255),
            claude: rgb(255, 153, 51),
            diff_added: rgb(153, 204, 255),
            diff_removed: rgb(255, 204, 204),
            diff_added_word: rgb(51, 102, 204),
            diff_removed_word: rgb(153, 51, 51),
        }
    }

    /// Dark ANSI — `theme.ts` `darkAnsiTheme` (named colors only).
    #[must_use]
    pub fn dark_ansi() -> Theme {
        Theme {
            text: ansi("ansi:whiteBright"),
            dim: ansi("ansi:white"),       // inactive
            error: ansi("ansi:redBright"),
            success: ansi("ansi:greenBright"),
            warning: ansi("ansi:yellowBright"),
            permission: ansi("ansi:blueBright"),
            plan_mode: ansi("ansi:cyanBright"),
            suggestion: ansi("ansi:blueBright"),
            claude: ansi("ansi:redBright"),
            diff_added: ansi("ansi:green"),
            diff_removed: ansi("ansi:red"),
            diff_added_word: ansi("ansi:greenBright"),
            diff_removed_word: ansi("ansi:redBright"),
        }
    }

    /// Light ANSI — `theme.ts` `lightAnsiTheme` (named colors only).
    #[must_use]
    pub fn light_ansi() -> Theme {
        Theme {
            text: ansi("ansi:black"),
            dim: ansi("ansi:blackBright"), // inactive
            error: ansi("ansi:red"),
            success: ansi("ansi:green"),
            warning: ansi("ansi:yellow"),
            permission: ansi("ansi:blue"),
            plan_mode: ansi("ansi:cyan"),
            suggestion: ansi("ansi:blue"),
            claude: ansi("ansi:redBright"),
            diff_added: ansi("ansi:green"),
            diff_removed: ansi("ansi:red"),
            diff_added_word: ansi("ansi:greenBright"),
            diff_removed_word: ansi("ansi:redBright"),
        }
    }
}

/// Registry lookup: resolve a [`ThemeName`] to its concrete palette.
#[must_use]
pub fn theme_for(name: ThemeName) -> Theme {
    match name {
        ThemeName::Dark => Theme::dark(),
        ThemeName::Light => Theme::light(),
        ThemeName::DarkDaltonized => Theme::dark_daltonized(),
        ThemeName::LightDaltonized => Theme::light_daltonized(),
        ThemeName::DarkAnsi => Theme::dark_ansi(),
        ThemeName::LightAnsi => Theme::light_ansi(),
    }
}
```

- [ ] **Step 2b: Verify against `theme.ts`** — open `claude-code/src/utils/theme.ts` and diff every ported triple against the matching key in `darkTheme`/`lightTheme`/`darkDaltonizedTheme`/`lightDaltonizedTheme`/`darkAnsiTheme`/`lightAnsiTheme`. Fix any mismatch. (The test pins the high-value ones; the rest must match the file exactly.)

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib theme::tests::registry_returns_distinct_palettes_with_locked_colors`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/crates/tui/src/theme.rs
git commit -m "plan(M7-15 T2): Theme struct + 6-theme registry (locked theme.ts colors)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: `AppState.theme` + `theme_setting` + `set_theme`

**Files:**
- Modify: `lingxi-code/crates/tui/src/state.rs`
- Test: `lingxi-code/crates/tui/src/state.rs` (inline tests)

- [ ] **Step 1: Write the failing test** — append to `state.rs` tests:

```rust
#[test]
fn app_state_carries_theme_and_set_theme_applies() {
    use crate::theme::{Theme, ThemeName, ThemeSetting};
    let mut s = AppState::new(fake_status());
    // Default: auto → resolves dark.
    assert_eq!(s.theme_setting, ThemeSetting::Auto);
    assert_eq!(s.theme, Theme::dark());
    // Switching applies both fields.
    s.set_theme(ThemeSetting::Named(ThemeName::Light));
    assert_eq!(s.theme_setting, ThemeSetting::Named(ThemeName::Light));
    assert_eq!(s.theme, Theme::light());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib state::tests::app_state_carries_theme_and_set_theme_applies`
Expected: FAIL — no `theme`/`theme_setting` field, no `set_theme`.

- [ ] **Step 3: Write minimal implementation**

In the `use` block of `state.rs`, add: `use crate::theme::{theme_for, Theme, ThemeSetting};`

Add two fields to `struct AppState` (near the `status` field):

```rust
    /// (M7-15) Active resolved render palette. Read by StatusLine, message
    /// renderers, and diff coloring. Mutated only via [`AppState::set_theme`].
    pub theme: Theme,
    /// (M7-15) Stored theme *preference* (`auto` + 6 names). `Auto` resolves
    /// to a concrete `ThemeName` for `theme`. Persisted to settings.json.
    pub theme_setting: ThemeSetting,
```

In `AppState::new`, initialize them (default `Auto`):

```rust
            theme: theme_for(ThemeSetting::Auto.resolve()),
            theme_setting: ThemeSetting::Auto,
```

Add the method to `impl AppState`:

```rust
    /// (M7-15) Apply a theme preference: store it and resolve the active
    /// palette. The caller persists the choice separately (Task 9).
    pub fn set_theme(&mut self, setting: ThemeSetting) {
        self.theme_setting = setting;
        self.theme = theme_for(setting.resolve());
    }
```

(`default_for_tests()` delegates to `new()` so it picks up the defaults automatically — no change needed there.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib state::tests`
Expected: PASS (new test + existing state tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/state.rs
git commit -m "plan(M7-15 T3): AppState.theme + theme_setting + set_theme()

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: `Screen::Theme` variant + `ThemePickerState` + key routing

Adds the picker as a screen on the M7-11 `active_screen` infra, routed at priority 2 in the §2.5 dispatcher (the single `handle_live_key`). **Grep first** for the M7-11 `Screen` enum + dispatcher branch (see Prerequisites). The code below assumes M7-11 named it `enum Screen` with `AppState.active_screen: Option<Screen>`; adapt to the real names.

**Files:**
- Create: `lingxi-code/crates/tui/src/screens/theme.rs`
- Modify: `lingxi-code/crates/tui/src/screens/mod.rs` (`pub mod theme;` + add `Theme` to `Screen`)
- Modify: `lingxi-code/crates/tui/src/root.rs` (route `Screen::Theme` keys; no parallel path)
- Test: `lingxi-code/crates/tui/tests/theme_picker_behavior.rs`

- [ ] **Step 1: Write the failing test** — create `tests/theme_picker_behavior.rs`:

```rust
use lingxi_tui::screens::theme::{theme_picker_handle_key, ThemePickerOutcome, ThemePickerState};
use lingxi_tui::state::AppState;
use lingxi_tui::theme::{Theme, ThemeName, ThemeSetting};
use crossterm::event::{KeyCode, KeyEvent};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn down_arrow_moves_highlight_and_live_previews() {
    let mut app = AppState::default_for_tests();
    let mut st = ThemePickerState::new(app.theme_setting); // starts on current setting
    let start = st.highlighted;
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Down));
    assert_eq!(out, ThemePickerOutcome::Stay);
    assert_eq!(st.highlighted, start + 1);
    // Live preview: app.theme now reflects the highlighted option (not yet committed setting).
    let previewed = ThemePickerState::OPTIONS[st.highlighted];
    assert_eq!(app.theme, lingxi_tui::theme::theme_for(previewed.resolve()));
}

#[test]
fn enter_commits_highlighted_setting() {
    let mut app = AppState::default_for_tests();
    let mut st = ThemePickerState::new(app.theme_setting);
    // Move to "light" (index 2 in OPTIONS: auto, dark, light, ...).
    st.highlighted = 2;
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Enter));
    assert_eq!(out, ThemePickerOutcome::Commit);
    assert_eq!(app.theme_setting, ThemeSetting::Named(ThemeName::Light));
    assert_eq!(app.theme, Theme::light());
}

#[test]
fn esc_cancels_and_restores_prior_theme() {
    let mut app = AppState::default_for_tests();
    let prior = app.theme_setting;
    let mut st = ThemePickerState::new(prior);
    theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Down)); // preview drift
    let out = theme_picker_handle_key(&mut st, &mut app, key(KeyCode::Esc));
    assert_eq!(out, ThemePickerOutcome::Cancel);
    // Restored to what it was before opening.
    assert_eq!(app.theme_setting, prior);
    assert_eq!(app.theme, lingxi_tui::theme::theme_for(prior.resolve()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test theme_picker_behavior`
Expected: FAIL — `screens::theme` module not present.

- [ ] **Step 3: Write minimal implementation** — create `screens/theme.rs`:

```rust
//! Theme picker screen (claude-code `ThemePicker.tsx` parity).
//!
//! Reached via the `/theme` command → `Screen::Theme`. Arrow keys move the
//! highlight and *live-preview* the theme on `AppState`; Enter commits the
//! setting; Esc cancels and restores the setting that was active on open.

use crossterm::event::{KeyCode, KeyEvent};

use crate::state::AppState;
use crate::theme::{theme_for, ThemeName, ThemeSetting};

/// Locked option labels (claude-code `ThemePicker.tsx`), aligned 1:1 with
/// [`ThemePickerState::OPTIONS`].
pub const OPTION_LABELS: [&str; 7] = [
    "Auto (match terminal)",
    "Dark mode",
    "Light mode",
    "Dark mode (colorblind-friendly)",
    "Light mode (colorblind-friendly)",
    "Dark mode (ANSI colors only)",
    "Light mode (ANSI colors only)",
];

/// What the key handler tells the caller to do with the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePickerOutcome {
    /// Stay open (highlight moved / preview changed).
    Stay,
    /// Enter — committed the highlighted setting; close the screen.
    Commit,
    /// Esc — restored the prior setting; close the screen.
    Cancel,
}

/// Picker state: the option list + highlight index + the setting to restore
/// on cancel.
#[derive(Debug, Clone)]
pub struct ThemePickerState {
    /// Highlighted index into [`Self::OPTIONS`] / [`OPTION_LABELS`].
    pub highlighted: usize,
    /// Setting active when the picker opened (restored on Esc).
    prior: ThemeSetting,
}

impl ThemePickerState {
    /// Option values, in `THEME_SETTINGS` order (auto first).
    pub const OPTIONS: [ThemeSetting; 7] = ThemeSetting::ALL;

    /// Open the picker focused on the currently-active setting.
    #[must_use]
    pub fn new(current: ThemeSetting) -> Self {
        let highlighted = Self::OPTIONS
            .iter()
            .position(|o| *o == current)
            .unwrap_or(0);
        Self {
            highlighted,
            prior: current,
        }
    }

    fn preview(&self, app: &mut AppState) {
        let setting = Self::OPTIONS[self.highlighted];
        // Preview mutates the active palette but NOT the stored preference,
        // so a cancel can cleanly restore.
        app.theme = theme_for(setting.resolve());
    }
}

/// Handle one key in the picker. Pure state transition (no I/O); the caller
/// owns persistence + closing the screen.
pub fn theme_picker_handle_key(
    state: &mut ThemePickerState,
    app: &mut AppState,
    key: KeyEvent,
) -> ThemePickerOutcome {
    match key.code {
        KeyCode::Up => {
            state.highlighted = state.highlighted.saturating_sub(1);
            state.preview(app);
            ThemePickerOutcome::Stay
        }
        KeyCode::Down => {
            let last = ThemePickerState::OPTIONS.len() - 1;
            state.highlighted = (state.highlighted + 1).min(last);
            state.preview(app);
            ThemePickerOutcome::Stay
        }
        KeyCode::Enter => {
            app.set_theme(ThemePickerState::OPTIONS[state.highlighted]);
            ThemePickerOutcome::Commit
        }
        KeyCode::Esc => {
            app.set_theme(state.prior);
            ThemePickerOutcome::Cancel
        }
        _ => ThemePickerOutcome::Stay,
    }
}

/// Pure render oracle: header + option rows (`❯ ` pointer on the highlight).
/// `_theme` is the live palette (color is applied in the component, not the
/// string oracle). Used by the snapshot test (Task 10).
#[must_use]
pub fn render_theme_picker_to_string(state: &ThemePickerState) -> String {
    let mut out = String::from("Theme\n");
    out.push_str("Choose the text style that looks best with your terminal\n");
    for (i, label) in OPTION_LABELS.iter().enumerate() {
        let pointer = if i == state.highlighted { "❯ " } else { "  " };
        out.push_str(pointer);
        out.push_str(label);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_align_with_options() {
        assert_eq!(OPTION_LABELS.len(), ThemePickerState::OPTIONS.len());
    }

    #[test]
    fn render_marks_highlighted_row() {
        let st = ThemePickerState {
            highlighted: 2,
            prior: ThemeSetting::Auto,
        };
        let s = render_theme_picker_to_string(&st);
        assert!(s.contains("❯ Light mode\n"));
        assert!(s.starts_with("Theme\n"));
    }
}
```

Add to `screens/mod.rs`: `pub mod theme;`. Add a `Theme` variant to the M7-11 `Screen` enum (whatever it is named) — e.g. `Theme(crate::screens::theme::ThemePickerState)` so the open screen carries its state; if M7-11's `Screen` is a fieldless enum with state living elsewhere on `AppState`, instead add `pub theme_picker_state: Option<ThemePickerState>` to `AppState` and a unit `Screen::Theme` variant. Match M7-11's established convention.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test theme_picker_behavior && cargo test -p lingxi-tui --lib screens::theme`
Expected: PASS.

- [ ] **Step 5: Route picker keys through the priority-2 branch in `root.rs`**

In `handle_live_key` (`root.rs`), the §2.5 priority order is: (1) `pending_permission`, (2) `active_screen`. The M7-11 branch for `active_screen.is_some()` already converts the iocraft key via `iocraft_to_crossterm028_key` and dispatches to the active screen. Add the `Screen::Theme` arm there:

```rust
    // (priority 2) active screen owns keys when open — M7-11 branch.
    if let Some(screen) = st.active_screen.as_mut() {
        match screen {
            // ... existing M7-11/12/13/14 screen arms ...
            Screen::Theme(picker) => {
                let ct = iocraft_to_crossterm028_key(k);
                let outcome = crate::screens::theme::theme_picker_handle_key(picker, st, ct);
                match outcome {
                    crate::screens::theme::ThemePickerOutcome::Commit => {
                        // Best-effort persist (Task 9), then close.
                        crate::theme_persist::save_theme_setting(st.theme_setting);
                        st.active_screen = None;
                    }
                    crate::screens::theme::ThemePickerOutcome::Cancel => {
                        st.active_screen = None;
                    }
                    crate::screens::theme::ThemePickerOutcome::Stay => {}
                }
            }
        }
        return;
    }
```

(`theme_persist::save_theme_setting` is created in Task 9. Until Task 9 lands, leave a `// TODO(M7-15 T9): persist` comment and just `st.active_screen = None`. The borrow of `st` is split: `picker` is `&mut` into the `Screen`, and `theme_picker_handle_key` takes `app: &mut AppState` — if M7-11 stored the picker state *inside* the `Screen` variant on `AppState`, you'll hit a double-mut-borrow; resolve it by storing `ThemePickerState` in a dedicated `AppState.theme_picker_state` slot and making `Screen::Theme` a unit variant, matching M6's `pending_permission` shape. Note this in the borrow comment.)

- [ ] **Step 6: Verify routing compiles + no parallel key path**

Run: `cargo check -p lingxi-tui`
Expected: clean. Confirm by inspection that the picker is reached ONLY through the priority-2 `active_screen` branch (no second key path) — this is the M6 focus-trap discipline (§2.5).

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/crates/tui/src/screens/theme.rs lingxi-code/crates/tui/src/screens/mod.rs lingxi-code/crates/tui/src/root.rs lingxi-code/crates/tui/tests/theme_picker_behavior.rs
git commit -m "plan(M7-15 T4): theme picker screen + priority-2 key routing

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Wire the `/theme` command to open the picker

claude-code's `/theme` command (`src/commands/theme/theme.tsx`) opens the ThemePicker. lingxi has a `theme` config field already; this task wires the `/theme` slash command to open `Screen::Theme`. **Grep first** for how existing slash commands open screens (M7-11/12/13 established this): `grep -rn "Screen::\|active_screen = Some\|\"theme\"\|/theme" lingxi-code/crates/tui/src/`.

**Files:**
- Modify: the command-dispatch site found by grep (likely `app.rs` or a commands handler in the TUI crate)
- Test: `lingxi-code/crates/tui/tests/theme_picker_behavior.rs` (add a command-open test) OR a behavior test at the dispatch site

- [ ] **Step 1: Write the failing test** — add to `theme_picker_behavior.rs`:

```rust
#[test]
fn slash_theme_opens_picker_screen() {
    let mut app = AppState::default_for_tests();
    // Drive the command dispatcher the same way the live REPL does.
    lingxi_tui::app::handle_slash_command(&mut app, "/theme");
    // The Theme screen is now active.
    assert!(app.active_screen.is_some());
    assert!(matches!(
        app.active_screen.as_ref().unwrap(),
        lingxi_tui::screens::Screen::Theme(_)
    ));
}
```

(Adapt `handle_slash_command`/`Screen::Theme(_)` to the real command-dispatch fn + Screen variant names found by grep. If the TUI routes `/theme` through the engine command surface rather than a local handler, instead assert the local "open screen" effect at whatever seam handles screen-opening commands.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test theme_picker_behavior slash_theme_opens_picker_screen`
Expected: FAIL — `/theme` does not open the screen yet.

- [ ] **Step 3: Write minimal implementation** — at the command-dispatch site, add a `/theme` arm that sets `active_screen = Some(Screen::Theme(ThemePickerState::new(app.theme_setting)))` (or sets the `theme_picker_state` slot + unit `Screen::Theme`, per the Task 4 shape decision). Mirror exactly how M7-11's `/doctor` opens the Doctor screen.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test theme_picker_behavior slash_theme_opens_picker_screen`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/app.rs lingxi-code/crates/tui/tests/theme_picker_behavior.rs
git commit -m "plan(M7-15 T5): /theme command opens the theme picker screen

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: syntect `.tmTheme` follows `AppState.theme`

Make M7-02's syntect highlighter pick its `.tmTheme` by active `ThemeName`. **Grep `render/syntax.rs` first** (Prerequisites) for the real highlight entrypoint + theme handling. The plan below assumes M7-02 used `syntect::highlighting::ThemeSet::load_defaults()` and a hardcoded theme name; adapt names.

**Files:**
- Modify: `lingxi-code/crates/tui/src/render/syntax.rs`
- Test: `lingxi-code/crates/tui/tests/theme_syntax_follows.rs`

- [ ] **Step 1: Write the failing test** — create `tests/theme_syntax_follows.rs`:

```rust
use lingxi_tui::render::syntax;
use lingxi_tui::theme::ThemeName;

#[test]
fn syntect_theme_changes_with_active_theme() {
    // Same code, two themes → different highlighted output.
    let code = "fn main() { let x = 1; }";
    let dark = syntax::highlight(code, "rust", ThemeName::Dark);
    let light = syntax::highlight(code, "rust", ThemeName::Light);
    // The two renders are not byte-identical (different tmTheme palettes).
    assert_ne!(
        format!("{dark:?}"),
        format!("{light:?}"),
        "dark and light highlight should differ"
    );
}

#[test]
fn tm_theme_for_maps_dark_and_light_to_different_themes() {
    let d = syntax::tm_theme_for(ThemeName::Dark);
    let l = syntax::tm_theme_for(ThemeName::Light);
    // syntect themes expose a `name`; the bundled dark/light themes differ.
    assert_ne!(d.name, l.name);
}
```

(Adapt `syntax::highlight(code, lang, theme_name)` to M7-02's real signature. If M7-02's entrypoint takes `&str`/no theme, this task adds the `ThemeName` parameter and updates its existing tests + call sites accordingly.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test theme_syntax_follows`
Expected: FAIL — `tm_theme_for` not defined / `highlight` doesn't take `ThemeName`.

- [ ] **Step 3: Write minimal implementation** — in `render/syntax.rs`, add a lazy `ThemeSet` and a `ThemeName -> &Theme` map (§4 R9 lazy-load; bundle only the curated set — syntect's `load_defaults()` ships ~7 themes, no extra files needed):

```rust
use std::sync::LazyLock;
use syntect::highlighting::{Theme as SynTheme, ThemeSet};

use crate::theme::ThemeName;

static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Pick the bundled syntect `.tmTheme` for the active TUI theme. Dark themes
/// map to a dark tmTheme; light themes to a light one. ANSI themes reuse the
/// dark/light mapping (terminal handles the ANSI palette).
#[must_use]
pub fn tm_theme_for(name: ThemeName) -> &'static SynTheme {
    let key = match name {
        ThemeName::Dark | ThemeName::DarkDaltonized | ThemeName::DarkAnsi => "base16-ocean.dark",
        ThemeName::Light | ThemeName::LightDaltonized | ThemeName::LightAnsi => {
            "base16-ocean.light"
        }
    };
    THEME_SET
        .themes
        .get(key)
        .unwrap_or_else(|| &THEME_SET.themes["base16-ocean.dark"])
}
```

Then change the highlight entrypoint to accept `name: ThemeName` and call `tm_theme_for(name)` instead of the M7-02 hardcoded theme. Update M7-02's existing call sites (markdown code-fence highlighting, StructuredDiff syntax coloring) to thread the active `ThemeName` — they receive it from `AppState.theme_setting.resolve()` at the `render_screen`/dispatcher seam (Task 7 threads `&Theme`; add `theme_name: ThemeName` alongside, or carry it on `Theme` — simplest: add a `pub name: ThemeName` field to `Theme` in Task 2's struct if needed, OR pass `theme_setting.resolve()` down explicitly).

> **Decision note for executor:** the cleanest thread is to pass `ThemeName` (not just `Theme`) to the highlight path. If threading two values is awkward, add a `name: ThemeName` field to the `Theme` struct (Task 2) and set it in each constructor. Pick one and keep it consistent — do not introduce a global mutable.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test theme_syntax_follows`
Expected: PASS. If `base16-ocean.light` is not in `load_defaults()`, grep `THEME_SET.themes.keys()` (print in a scratch test) and pick the actual bundled light theme name (e.g. `"InspiredGitHub"` or `"Solarized (light)"`); update the map + test to the real names.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/render/syntax.rs lingxi-code/crates/tui/tests/theme_syntax_follows.rs
git commit -m "plan(M7-15 T6): syntect .tmTheme follows the active theme

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Thread `&Theme` into renderers; resolve `TODO(M7-15)` color sites

Centralize hardcoded colors into `Theme`. **Grep first:** `grep -rn "TODO(M7-15)" lingxi-code/crates/tui/src/` and `grep -rn "TuiTheme::" lingxi-code/crates/tui/src/`.

**Files:**
- Modify: `lingxi-code/crates/tui/src/app.rs` (`render_screen` passes `&state.theme` to the message dispatcher + StatusLine)
- Modify: `lingxi-code/crates/tui/src/components/scrollback.rs` (`render_message` takes/forwards `&Theme`)
- Modify: `lingxi-code/crates/tui/src/components/messages/*.rs` (each renderer takes a `color: Color` / `theme: &Theme` instead of `TuiTheme::*` consts where it had a `TODO(M7-15)` or a hardcoded literal color)
- Modify: `lingxi-code/crates/tui/src/render/diff.rs` (M7-02 diff colors read `theme.diff_added`/`diff_removed`/word variants)
- Test: `lingxi-code/crates/tui/tests/theme_snapshots.rs` (covered in Task 10) + an assertion test below

- [ ] **Step 1: Write the failing test** — create/append `tests/theme_todo_resolved.rs`:

```rust
// Guard: no TODO(M7-15) markers remain in the TUI source tree.
#[test]
fn no_m7_15_todo_markers_remain() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    visit(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "unresolved TODO(M7-15) markers: {offenders:?}"
    );

    fn visit(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                visit(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let body = std::fs::read_to_string(&p).unwrap_or_default();
                if body.contains("TODO(M7-15)") {
                    out.push(p.display().to_string());
                }
            }
        }
    }
}
```

Also add a behavior assertion that a renderer reads the theme (pick a renderer that had a `TODO(M7-15)`, e.g. the warning path in `system_text.rs`):

```rust
#[test]
fn system_text_warning_reads_theme_warning_color() {
    use lingxi_tui::theme::Theme;
    // The renderer's color helper returns theme.warning for the warning level.
    let dark = Theme::dark();
    let light = Theme::light();
    assert_ne!(dark.warning, light.warning); // sanity: themes differ
    // (The renderer-level assertion checks the chosen color equals theme.warning;
    //  adapt to the renderer's pure color-selection fn once M7-04 is merged.)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test theme_todo_resolved`
Expected: FAIL if any `TODO(M7-15)` markers exist (they will, once M7-04/05 are merged). If M7-04/05 are NOT yet merged, this passes trivially — document that the M7-16 final review re-runs it.

- [ ] **Step 3: Write minimal implementation**

For each `TODO(M7-15)` site (warning→`theme.warning`, success→`theme.success`, planMode→`theme.plan_mode`, etc.), replace the hardcoded `Color::Yellow`/`Color::Green`/`Color::Magenta` with the matching `theme.<field>`. Thread `&Theme` from `render_screen`:
- `app.rs::render_screen` — pull `let theme = &state.theme;` and pass it to the message dispatcher (`scrollback::render_message(..., theme)`) and to `StatusLine` (Task 8).
- `scrollback.rs::render_message` — add a `theme: &Theme` parameter; forward to each renderer component.
- Each message renderer component — accept the relevant color(s) (or `&Theme`) as a prop/arg; the pure `render_*_to_string` oracle is unaffected (it only produces text). Where M6 used `TuiTheme::ASSISTANT`, switch to `theme.claude`; `TuiTheme::ERROR` → `theme.error`; `TuiTheme::DIM` → `theme.dim`; `TuiTheme::USER` (terminal default) stays `Color::Reset` (claude-code renders user text uncolored; there is no theme key for it).
- `render/diff.rs` — diff add/remove/word colors read `theme.diff_added`/`diff_removed`/`diff_added_word`/`diff_removed_word`.

Keep the `TuiTheme` shim consts pointing at `Theme::dark()` values so any not-yet-migrated test compiles, but remove `TuiTheme::` usages from production render paths (the snapshot in Task 10 proves the live path uses `state.theme`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lingxi-tui --test theme_todo_resolved && cargo test -p lingxi-tui`
Expected: PASS; no `TODO(M7-15)` markers remain; existing renderer snapshots may need `cargo insta review` if a color changed from the M7-04 literal to the (identical-valued) theme field — verify the bytes are unchanged for the default dark theme (they should be: `theme.warning` dark = the same yellow family, but if the RGB differs from `Color::Yellow`, the snapshot legitimately changes — accept it as the theme-correct value and re-bless).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/app.rs lingxi-code/crates/tui/src/components lingxi-code/crates/tui/src/render/diff.rs lingxi-code/crates/tui/tests/theme_todo_resolved.rs
git commit -m "plan(M7-15 T7): thread &Theme into renderers; resolve TODO(M7-15) colors

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: StatusLine reads theme colors

**Files:**
- Modify: `lingxi-code/crates/tui/src/components/status_line.rs`
- Modify: `lingxi-code/crates/tui/src/app.rs` (`render_screen` passes the theme to `StatusLine`)
- Test: `lingxi-code/crates/tui/src/components/status_line.rs` (inline) + the snapshot in Task 10

- [ ] **Step 1: Write the failing test** — append to `status_line.rs` tests:

```rust
#[test]
fn status_line_props_carry_theme() {
    use crate::theme::Theme;
    // StatusLineProps gains a `theme` field; default is dark.
    let props = StatusLineProps::default();
    assert_eq!(props.theme, Theme::dark());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --lib components::status_line::tests::status_line_props_carry_theme`
Expected: FAIL — no `theme` field on `StatusLineProps`.

- [ ] **Step 3: Write minimal implementation**

Add `pub theme: Theme` to `StatusLineProps` (import `use crate::theme::Theme;`); default to `Theme::dark()` in `impl Default`. In the `StatusLine` component, color the rendered line segments from `props.theme` (the M6 status line was a single uncolored `Text`; keep `format_status_line` byte-identical for the literal-lock, but apply `color: props.theme.text` to the `Text`, and if any segment was specially colored, source it from the theme). In `app.rs::render_screen`, pass `theme: state.theme` into the `StatusLine` element.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --lib components::status_line`
Expected: PASS (new test + the existing `format_matches_byte_locks` / `mode_label_covers_all_variants`, which are unaffected — the format string is unchanged).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/components/status_line.rs lingxi-code/crates/tui/src/app.rs
git commit -m "plan(M7-15 T8): StatusLine colors read from the active Theme

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Best-effort theme persistence (settings.json `theme` field)

No new persistence logic (§4 R7): reuse the existing `~/.claude/settings.json` `theme` field. **Grep first** for the existing path-resolution + read-modify-write helpers the config tool uses: `grep -rn "settings.json\|home_dir\|\.claude\|read_settings_obj\|write_settings_obj\|CONFIG_FIELD_THEME" lingxi-code/crates/tools/src/builtin/config.rs lingxi-code/crates/core/src/settings/`. Reuse `Settings::load`/`EffectiveSettings` for the read.

**Files:**
- Create: `lingxi-code/crates/tui/src/theme_persist.rs` (tiny module: load + save the theme wire string)
- Modify: `lingxi-code/crates/tui/src/lib.rs` (`mod theme_persist;`)
- Modify: the session/startup site that constructs `AppState` (grep `AppState::new(`) — load the stored theme at startup
- Test: `lingxi-code/crates/tui/tests/theme_persist.rs`

- [ ] **Step 1: Write the failing test** — create `tests/theme_persist.rs`:

```rust
use lingxi_tui::theme::ThemeSetting;
use lingxi_tui::theme_persist;

#[test]
fn save_then_load_roundtrips_via_settings_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    // Save a setting to an explicit path (test-injected).
    theme_persist::save_theme_setting_to(&path, ThemeSetting::Named(
        lingxi_tui::theme::ThemeName::Light,
    ))
    .unwrap();
    // The JSON object carries `"theme": "light"`.
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"theme\""));
    assert!(body.contains("\"light\""));
    // Load reads it back.
    let loaded = theme_persist::load_theme_setting_from(&path);
    assert_eq!(loaded, Some(ThemeSetting::Named(lingxi_tui::theme::ThemeName::Light)));
}

#[test]
fn save_preserves_other_settings_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{\n  \"model\": \"claude-sonnet-4.5\"\n}\n").unwrap();
    theme_persist::save_theme_setting_to(&path, ThemeSetting::Named(
        lingxi_tui::theme::ThemeName::Dark,
    ))
    .unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"model\""));   // untouched
    assert!(body.contains("\"theme\""));   // added
}
```

(Add `tempfile` to the tui crate's `[dev-dependencies]` if not present — `grep tempfile lingxi-code/crates/tui/Cargo.toml`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lingxi-tui --test theme_persist`
Expected: FAIL — `theme_persist` module not present.

- [ ] **Step 3: Write minimal implementation** — create `theme_persist.rs`:

```rust
//! Best-effort theme persistence via `~/.claude/settings.json` `theme` field.
//!
//! No new persistence engine (spec §4 R7): this read-modify-writes the same
//! JSON object the existing `ConfigTool` allowlists. If the home dir or file
//! is unavailable, save/load degrade to a no-op and the theme stays
//! session-only — the picker still applies it live.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::theme::ThemeSetting;

/// Resolve `~/.claude/settings.json` (the same target the config tool uses).
#[must_use]
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// Read the stored theme setting, if any. Returns `None` on any error
/// (missing file, parse failure, absent/unknown `theme` value).
#[must_use]
pub fn load_theme_setting() -> Option<ThemeSetting> {
    load_theme_setting_from(&settings_path()?)
}

/// Test seam: read from an explicit path.
#[must_use]
pub fn load_theme_setting_from(path: &Path) -> Option<ThemeSetting> {
    let body = std::fs::read_to_string(path).ok()?;
    let obj: Map<String, Value> = serde_json::from_str(&body).ok()?;
    let wire = obj.get("theme")?.as_str()?;
    ThemeSetting::from_wire(wire)
}

/// Best-effort save. Logs + swallows errors (theme stays session-only).
pub fn save_theme_setting(setting: ThemeSetting) {
    let Some(path) = settings_path() else {
        tracing::debug!("theme persist skipped: no home dir");
        return;
    };
    if let Err(e) = save_theme_setting_to(&path, setting) {
        tracing::debug!(error = %e, "theme persist failed (session-only)");
    }
}

/// Test seam: read-modify-write the `theme` field at an explicit path,
/// preserving all other keys. Pretty JSON + trailing newline (config-tool
/// shape).
pub fn save_theme_setting_to(path: &Path, setting: ThemeSetting) -> std::io::Result<()> {
    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    obj.insert("theme".to_string(), Value::String(setting.as_wire().to_string()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&obj)?;
    body.push('\n');
    std::fs::write(path, body)
}
```

(Confirm `dirs` is a dep of the tui crate — `grep -n "dirs" lingxi-code/crates/tui/Cargo.toml`; if not, use whatever home-dir crate the config tool uses, found in Step's grep. Do NOT add a new dep family — reuse the existing one.)

Add `pub mod theme_persist;` to `lib.rs`. At the `AppState::new(` startup site, after constructing the state, apply any stored setting: `if let Some(s) = crate::theme_persist::load_theme_setting() { state.set_theme(s); }`. Wire the `save_theme_setting` call into Task 4's Commit arm (replace the `// TODO(M7-15 T9): persist` note).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test theme_persist`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/src/theme_persist.rs lingxi-code/crates/tui/src/lib.rs lingxi-code/crates/tui/src/root.rs lingxi-code/crates/tui/Cargo.toml lingxi-code/crates/tui/tests/theme_persist.rs
git commit -m "plan(M7-15 T9): best-effort theme persistence via settings.json

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: Snapshots — StatusLine + message + code block under dark & light

**Files:**
- Create: `lingxi-code/crates/tui/tests/theme_snapshots.rs`
- Snapshots: `lingxi-code/crates/tui/tests/snapshots/` (insta auto-creates)

- [ ] **Step 1: Write the failing test** — create `tests/theme_snapshots.rs`. Use the same render entrypoint the live mount uses (`app::render_screen` against an `AppState` with a fixed scrollback + a fenced code block), once per theme:

```rust
use lingxi_tui::state::{AppState, RenderedMessage};
use lingxi_tui::theme::{ThemeName, ThemeSetting};

fn fixture_state(theme: ThemeName) -> AppState {
    let mut s = AppState::default_for_tests();
    s.set_theme(ThemeSetting::Named(theme));
    s.status.model = "claude-sonnet-4.5".into();
    s.push_message(RenderedMessage::UserText { body: "hi".into(), timestamp: 0 });
    s.push_message(RenderedMessage::AssistantText {
        body: "Here is code:\n```rust\nfn main() { let x = 1; }\n```".into(),
        timestamp: 0,
    });
    s
}

#[test]
fn statusline_message_codeblock_dark() {
    let s = fixture_state(ThemeName::Dark);
    // Render to a stable textual form. Use the project's established render-
    // to-string oracle for screens (grep render_screen_to_string / a test
    // helper from M7-03/M7-04 snapshot tests) so the snapshot is deterministic.
    let rendered = lingxi_tui::app::render_screen_to_string(&s, 24);
    insta::assert_snapshot!("theme_dark_full", rendered);
}

#[test]
fn statusline_message_codeblock_light() {
    let s = fixture_state(ThemeName::Light);
    let rendered = lingxi_tui::app::render_screen_to_string(&s, 24);
    insta::assert_snapshot!("theme_light_full", rendered);
}
```

> **Note:** `app::render_screen` returns an iocraft `AnyElement` (not a string). Snapshot tests in M7-03/M7-04 used the pure `render_*_to_string` oracles. **Grep** for the screen-level string oracle these prior tests used (`grep -rn "render_screen_to_string\|render_entry_to_string\|to_string(&" lingxi-code/crates/tui/tests/`). If none exists at the screen level, snapshot the **composition of pure oracles** instead: `format_status_line(...)` + `render_entry_to_string(...)` per message (these already exist) + the markdown/syntax `render::*` pure fns for the code block — concatenated. The point of the snapshot is that the *colors* differ between themes; since the pure string oracles drop color, ALSO assert the color-bearing path differs via the Task 6 syntax test + a `Debug`-format comparison of the styled lines for dark vs light (add `assert_ne!` on the two themes' styled-line debug output, mirroring `theme_syntax_follows.rs`). Keep the textual insta snapshot for layout/label regressions and the `assert_ne!` for color divergence.

- [ ] **Step 2: Run test to verify it fails / creates snapshots**

Run: `cargo test -p lingxi-tui --test theme_snapshots`
Expected: FAIL first run (no `.snap` yet) — insta writes `.snap.new`.

- [ ] **Step 3: Review + accept snapshots**

Run: `cd lingxi-core && cargo insta review` (accept the two snapshots after eyeballing: dark shows the dark labels/code, light shows the light variants; layout matches the live REPL chrome).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lingxi-tui --test theme_snapshots`
Expected: PASS (snapshots blessed).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/tui/tests/theme_snapshots.rs lingxi-code/crates/tui/tests/snapshots/
git commit -m "plan(M7-15 T10): snapshots — statusline + message + code block under dark/light

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 11: Workspace gate + tag `m7.15`

**Files:** none (verification + tag only)

- [ ] **Step 1: Format + lint + full test (from inside `lingxi-code/`)**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: clean fmt; zero clippy warnings; all tests pass. Known flakes (allowed rerun): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing. If one trips, rerun that test once.

- [ ] **Step 2: Telemetry baseline unchanged (0 added)**

```bash
cargo test --workspace all_event_names 2>/dev/null || true
grep -rn "ALL_EVENT_NAMES" lingxi-code/crates/telemetry/src/ | head
```

Expected: `ALL_EVENT_NAMES.len()` still **326** (M7-15 registered no events). Confirm no new `tengu_tui_*` constant was added by this plan.

- [ ] **Step 3: Cross-platform compile gate (5 targets)**

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

Expected: green for all 5 (same posture as v0.7.0). `theme.rs`/`syntax.rs`/`theme_persist.rs` are pure std + iocraft + syntect + serde — no platform-specific code. If a target's toolchain isn't installed, note it and skip per the v0.7.0 convention.

- [ ] **Step 4: Tag `m7.15`**

```bash
git tag -a m7.15 -m "M7-15 — theme picker: Theme registry (6 themes) + /theme picker screen + syntect tmTheme follows active theme + centralized TODO(M7-15) colors"
```

(NO push to remote. Local tag only — parent spec §6.4.)

- [ ] **Step 5: Verify the tag exists locally**

```bash
git tag -l 'm7.15'
git show --stat m7.15 | head -20
```

Expected: `m7.15` listed; `git show` points at the T10 (or T11 fmt/clippy) commit.

---

## Self-Review

**1. Spec coverage** — mapping the prompt's "WHAT M7-15 SHIPS" + "TESTS REQUIRED" to tasks:

| Prompt requirement | Task |
|---|---|
| `Theme` struct + registry of named themes (claude-code's dark/light + named set) | T1 (`ThemeName`/`ThemeSetting`/`ansi` map), T2 (`Theme` struct + `theme_for` registry, 6 themes, locked colors) |
| Active theme in `AppState.theme` | T3 (`theme` + `theme_setting` + `set_theme`) |
| Theme picker (screen via `active_screen`; match `ThemePicker.tsx`); arrow-select; preview; Enter applies; live re-render across StatusLine/message/diff colors | T4 (`Screen::Theme` + `ThemePickerState` + priority-2 routing + preview/commit/cancel), T5 (`/theme` opens it), T7+T8 (live colors), T10 (snapshot proof) |
| Code-block highlighting maps active theme → syntect `.tmTheme`; switching theme switches syntect theme | T6 (`tm_theme_for` + threaded `ThemeName`) |
| Resolve `TODO(M7-15)` markers; centralize into `Theme` | T7 (grep + resolve + guard test) |
| Snapshot: StatusLine + message + code block under ≥2 themes (dark+light) | T10 |
| Behavior: picker lists/select+Enter sets `AppState.theme`/reflected | T4 |
| Behavior: syntect theme follows active theme | T6 |
| Behavior: `TODO(M7-15)` colors now read from `Theme` | T7 |
| Theme persistence through existing store (M3 settings); else session + note | T9 (best-effort `settings.json` `theme` field; session-only fallback documented) |
| §4 R9 curated set + lazy-load syntect | T2 (6 curated themes only), T6 (`LazyLock<ThemeSet>`) |
| Telemetry 326, +0 events | T11 Step 2 (no event registered anywhere) |
| Workspace gate (cd lingxi-core) + tag `m7.15` | T11 |

**2. Placeholder scan:** searched the plan for `TBD`, `implement later`, `fill in details`, `add appropriate`, `handle edge cases`, `similar to Task` — none. Every code step shows complete code. The intentional `TODO(M7-15)`/`TODO(M7-15 T9)` strings are the *targets being resolved* (T7 grep + a guard test, T4→T9 wiring), not plan placeholders.

**3. Type consistency:** `ThemeName`, `ThemeSetting`, `Theme`, `ansi`, `theme_for`, `set_theme`, `ThemePickerState`, `ThemePickerOutcome` (`Stay`/`Commit`/`Cancel`), `theme_picker_handle_key`, `render_theme_picker_to_string`, `OPTION_LABELS`, `ThemePickerState::OPTIONS`, `tm_theme_for`, `theme_persist::{load_theme_setting,save_theme_setting,save_theme_setting_to,load_theme_setting_from}` are each defined once and referenced consistently. `Theme` derives `Copy`/`PartialEq` so `assert_eq!` on it (T2/T3/T8) and the `==` in `ThemePickerState::new` work. `ThemeSetting::ALL` (7) drives both the picker options and the wire roundtrip; `ThemeName::ALL` (6) drives the registry.

**Notes / decisions for the executor:**
- **Picker is a screen, reached via `/theme`** — this satisfies both halves of spec §2.4 ("a screen via active_screen, OR a `/theme` overlay") and matches claude-code (`/theme` command → `ThemePicker.tsx`). It rides M7-11's `active_screen` infra at priority 2 (§2.5), so it inherits the focus-trap discipline for free and adds no parallel key path. The Task 4 escape hatch (a `pending_theme_picker` slot mirroring `pending_permission`) covers the case where M7-11 hasn't merged.
- **`auto` resolves to `dark` headless.** No terminal-background probe in M7 (deferred to M8). `auto` is offered as the first picker option for claude-code parity and stored faithfully, but resolves to dark at render time.
- **Theme persistence is best-effort via the existing `settings.json` `theme` field** (already an allowlisted config field). No new persistence engine (§4 R7). If home dir / file is unavailable the theme is session-only and the picker still applies it live — documented in `theme_persist.rs`. If M7-13 (Settings screen) later exposes a typed `Settings` write API, T9 can be refactored to call it, but this plan does NOT depend on M7-13.
- **`Theme` carries only the keys the lingxi TUI renders** (~13 of claude-code's ~80 `Theme` keys). The other keys (shimmer, rainbow, agent colors, TUI-V2 backgrounds) are out of scope until the renderers that consume them exist. Add fields incrementally when a renderer needs one — do not port the whole struct.
- **The `TuiTheme` shim stays** (delegating to `Theme::dark()` values) so any straggler test compiles, but production render paths are migrated off it in T7/T8. M7-16 can delete the shim once nothing references it.

**Gaps surfaced (raise before execution if blocking):**
1. **M7-02's real `render/syntax.rs` API is unknown at authoring time** (M7-02 not yet merged). T6 explicitly greps and adapts; the `highlight(code, lang, ThemeName)` signature + `base16-ocean.dark`/`.light` theme names are assumptions to verify against the merged M7-02 (`load_defaults()` bundled theme names + actual entrypoint signature).
2. **M7-11's `Screen` enum shape is unknown** (M7-11 not yet merged). T4/T5 grep and adapt; the double-mut-borrow caveat (picker state inside `Screen` vs. a dedicated `AppState` slot) is flagged with the M6 `pending_permission` shape as the fallback.
3. **`TODO(M7-15)` markers only exist once M7-04/05 merge** (confirmed against the M7-04 plan's color table). If they're not merged when this executes, T7's guard test passes trivially and the M7-16 final review re-runs the grep — documented in Prerequisites + T7 Step 2.
4. **`dirs` / home-dir crate** for `theme_persist` must already be a tui-crate dep; T9 greps and reuses the config tool's existing dependency rather than adding a new one.

**End of M7-15 plan.**
