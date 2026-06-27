# Theme & Color Foundation — Design Spec

- **Date:** 2026-06-28
- **Status:** Approved (design); ready for implementation planning
- **Track:** T1 of 3 (see "Bigger picture" below)
- **Owner area:** `tui` crate (theme/terminal foundation)

## Context

The `/connect` UI overhaul surfaced that its visible problems (white-on-white
text on light terminals, colors quantizing to the wrong color on low-color
terminals) are not connect-specific — they are app-wide root causes in the
theme/terminal layer. Decomposing the connect-UI request produced three tracks:

- **T1 — Theme & color foundation (this spec).** Auto-detect the terminal
  background and color depth so the right theme is chosen on every terminal,
  app-wide.
- **T2 — Connect UI overhaul.** Data-driven providers, trustworthy `✓` state,
  real per-provider login methods + detail, unified visual flow. Builds on T1
  for color correctness. (Separate spec.)
- **T3 — Warp / alt-screen rendering.** Warp's block model mis-renders
  alt-screen TUIs (ghosting); fixing it changes the rendering model
  (inline vs alt-screen). Large, risky, unverifiable without a real Warp.
  (Separate spec, likely last.)

T1 was chosen to design first because it is the root cause of most of the
color/visibility issues and unblocks correct colors for T2 and every other
screen.

### Current behaviour (the problem)

`ThemeSetting::Auto.resolve()` (`tui/src/theme.rs`) consults only `$COLORFGBG`
and falls back to `Dark` when it is absent. Most macOS terminals
(Terminal.app, iTerm2 default) do not set `$COLORFGBG`, so a user on a **light**
terminal silently gets the **Dark** palette, whose `text = rgb(255,255,255)`
renders white-on-white (invisible). The OSC-11 terminal-background query that
claude-code uses (`utils/systemTheme.ts`, `ink/terminal-querier.ts`) was never
ported (it is explicitly deferred in a comment at `theme.rs`).

Separately, the app emits 24-bit truecolor SGR unconditionally; on a
non-truecolor terminal the terminal quantizes those colors to the nearest ANSI
slot, which can land on the wrong color (the "red bars" class seen in the
`/connect` picker).

### Key enabling discovery

The theme system already defines **six** themes — `Dark`, `Light`,
`LightDaltonized`, `DarkDaltonized`, and crucially **`DarkAnsi` / `LightAnsi`**
(16-color-safe, via an existing `ansi:<name>` → iocraft `Color` mapper). The
degradation *targets already exist*; T1 is mostly **wiring** — selecting the
right one — not authoring new palettes.

## Goals

1. On startup, detect the terminal **background** (light vs dark) via an OSC-11
   query, so `Auto` resolves correctly without the user running `/theme`.
2. Detect terminal **color depth** (truecolor vs low-color) and degrade to the
   existing `-ansi` themes on non-truecolor terminals, so colors render as exact
   named ANSI values instead of quantizing to the wrong color.
3. Be robust: never hang, never corrupt the screen, always fall back to a safe
   default (current behaviour) when detection is impossible.
4. App-wide: every screen (connect, model, REPL, …) benefits; no per-screen
   patching.

## Non-goals

- The connect UI data/visual overhaul (**T2**).
- The Warp / alt-screen rendering rework (**T3**).
- Live re-detection mid-session. **Decision: detect once at startup.** If the
  user switches their terminal's light/dark mode mid-session, they re-run or use
  `/theme`. (Chosen for simplicity + robustness; matches most TUIs.)
- Authoring new themes or changing the existing palettes / the `ansi:` mapper.

## Decisions made (during brainstorming)

- **Query mechanism: hand-rolled pre-flight OSC-11** (not the `termbg` crate).
  Dependency-free, full control, parity-shaped (mirrors claude-code's
  `terminal-querier`). Trade-off: termbg's tmux/screen passthrough handling is
  given up (see "Known limitations").
- **Detection cadence: once at startup.**
- **Degradation target:** the existing `DarkAnsi` / `LightAnsi` themes.

## Design

### Data flow

```
run_tui_session / run_resume_picker (startup, TTY-guarded, before iocraft mount)
  └─ detect_terminal_theme()                     [new, tui/src/theme_detect.rs]
       ├─ not a TTY?           → return (no detection)
       ├─ enter raw mode (brief)
       ├─ write  ESC ] 11 ; ? BEL   to stdout, flush
       ├─ read stdin until  ESC ] 11 ; rgb:RRRR/GGGG/BBBB  (BEL or ST), deadline ~100ms
       ├─ parse rgb (16-bit/chan → 0..1), BT.709 luminance = 0.2126R+0.7152G+0.0722B
       │     luminance > 0.5 → Light  else → Dark
       ├─ exit raw mode (iocraft re-enters on mount)
       └─ cache detected background  → set_detected_background(Some(Light|Dark))

ThemeSetting::Auto.resolve()                      [modified, tui/src/theme.rs]
  ├─ background = detected_background()            (OSC-11 cache)
  │              ?? colorfgbg_theme($COLORFGBG)    (synchronous seed, existing)
  │              ?? Dark                           (final fallback)
  ├─ depth      = truecolor if $COLORTERM ∈ {truecolor, 24bit} else low-color
  └─ pick:  (Dark, truecolor)  → Dark
            (Light, truecolor) → Light
            (Dark, low-color)  → DarkAnsi
            (Light, low-color) → LightAnsi
```

From the first rendered frame, the theme matches the terminal's background and
color depth. `/theme <name>` continues to set `ThemeSetting::Named(..)`, which
bypasses all of the above (unchanged).

### Components

- **`tui/src/theme_detect.rs` (new).**
  - `detect_terminal_theme()` — the OSC-11 pre-flight (raw-mode + write + timed
    read + cache). The only part doing terminal I/O.
  - Pure helpers (unit-testable, no I/O):
    - `parse_osc11_rgb(reply: &str) -> Option<(f64, f64, f64)>` — parse
      `rgb:RRRR/GGGG/BBBB` (also tolerate 8-bit `rgb:RR/GG/BB`), normalize to
      0..1.
    - `luminance_is_light(r, g, b) -> bool` — BT.709, `> 0.5`.
  - Cache: a process-global `OnceLock<Option<ThemeName>>` (background only:
    `Light`/`Dark`); `set_detected_background` / `detected_background`.
- **`tui/src/theme.rs` (modify).**
  - `ThemeSetting::Auto.resolve()` — consult `detected_background()` first, then
    the existing `colorfgbg_theme`, then `Dark`; combine with depth.
  - `color_depth() -> ColorDepth` — pure, reads `$COLORTERM` (and optionally
    `TERM` containing `direct`/`truecolor`). `{Truecolor, Low}`.
  - `resolve_theme(background: ThemeName, depth: ColorDepth) -> ThemeName` —
    the pure 4-cell mapper.
- **`tui/src/session.rs` (wire).**
  - Call `crate::theme_detect::detect_terminal_theme()` once at the top of
    `run_tui_session` and `run_resume_picker`, beside
    `install_terminal_safety_hooks()`, before building `AppState`.
- **Untouched:** the 6 `ThemeName` palettes, the `ansi:<name>` → `Color` mapper,
  the `/theme` picker + persistence, `ThemeName::to_wire`/`from_wire` (byte-locked).

### Color degradation detail

Selecting `DarkAnsi`/`LightAnsi` means the theme's colors are `ansi:<name>`
values mapped to crossterm named `Color`s; iocraft then emits named-color SGR
(`ESC[37m`, `ESC[90m`, …) instead of 24-bit `ESC[38;2;…`. The terminal renders
exact ANSI colors — no truecolor → nearest-slot quantization, so the
"wrong color on low-color terminals" failure mode is removed for those
terminals.

Depth detection precedence:
1. `$COLORTERM` is `truecolor` or `24bit` → Truecolor.
2. else `$TERM` contains `direct` or `truecolor` → Truecolor.
3. else → Low (covers `xterm-256color`, `screen`, `tmux`, 16-color, unset).

(256-color terminals are treated as Low: there is no 256-color palette, and the
16-color `-ansi` themes render exactly on them. This is intentional — exactness
over a richer-but-quantized palette.)

### Error handling / fallbacks

| Situation | Behaviour |
|---|---|
| stdout/stdin not a TTY (pipe, CI) | Skip detection entirely → Auto → `$COLORFGBG` → Dark (current). |
| OSC-11 no reply within ~100 ms (unsupported terminal) | Hard deadline; fall back to `$COLORFGBG` → Dark. Never hangs. |
| Malformed / partial reply | Ignored → fallback. |
| `enable_raw_mode()` fails | Skip detection → fallback. |
| Stray query/reply bytes | Consumed by the terminal / read off stdin **before** iocraft mounts; the first iocraft frame overwrites any residue. |
| User ran `/theme <name>` previously | `ThemeSetting::Named` — detection is not consulted (explicit override wins). |

The ~100 ms timeout is a starting value (terminals reply in ~1–50 ms); it is a
single tunable constant.

### Testing

**Pure unit tests (deterministic, no I/O):**
- `parse_osc11_rgb`: `rgb:ffff/ffff/ffff` → white; `rgb:0000/0000/0000` → black;
  8-bit `rgb:ff/80/00`; malformed inputs → `None`.
- `luminance_is_light`: white → light, black → dark, a value either side of the
  0.5 boundary.
- `resolve_theme` 4-cell table: every `(background, depth)` → expected
  `ThemeName`.
- `color_depth`: `COLORTERM=truecolor` → Truecolor; unset / `xterm-256color` /
  `screen` → Low; `TERM=*-direct` → Truecolor.
- `Auto.resolve()` precedence: detected-background wins over `$COLORFGBG` wins
  over Dark.

**Integration (PTY, harness plays the terminal):**
- pexpect spawns `lingxi-cli`, watches stdout for the `ESC]11;?` query, writes
  back a **light** reply (`ESC]11;rgb:ffff/ffff/ffff`) → assert the picker/title
  renders with the **Light** theme (dark text on the framebuffer).
- Same with a **dark** reply → **Dark** theme.
- **No-reply** run → assert the process does not hang and falls back to Dark.

This is the part pyte alone cannot do (pyte does not answer OSC queries), so the
test supplies the reply. (This mirrors the existing PTY-driver pattern.)

**Regression:** existing theme tests and the byte-locked `to_wire`/`from_wire`
stay green — the palettes and wire strings are untouched.

## Known limitations

- **tmux / screen:** OSC-11 frequently needs passthrough configuration inside a
  multiplexer; the hand-rolled query may not get a reply there, so light/dark
  falls back to `$COLORFGBG` → Dark (user can `/theme`). Color-**depth**
  degradation still applies (driven by `COLORTERM`/`TERM`), so tmux still gets
  the `-ansi` themes. (termbg handles passthrough; we traded that for
  control/parity.)
- **Mid-session terminal theme change** is not followed (detect-once-at-startup
  decision).
- T1 does not address Warp's alt-screen ghosting (T3).

## Bigger picture (tracks, for reference)

1. **T1 (this spec)** — theme/color foundation. ← implement first.
2. **T2** — connect UI overhaul (data-driven providers, real `✓`, real login
   methods, real detail, unified visual). Depends on T1 for colors.
3. **T3** — Warp / alt-screen rendering model. Independent, hardest, last.

Each track gets its own spec → implementation plan → build cycle.
