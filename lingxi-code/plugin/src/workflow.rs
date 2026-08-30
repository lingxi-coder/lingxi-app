//! Plugin workflow inventory + the named-workflow resolver (spec §5.5 /
//! §19.1, `LOCAL-APP-PLUGIN-DESIGN-V2.md:395-427`).
//!
//! `discovery::detect_components` (P0a.2) already fills
//! `PluginComponents::workflows` with the `.js`-only inventory of a plugin's
//! `workflows/` directory (or its manifest override) — see [`crate::discovery`]'s
//! `glob_js` / `resolve_workflow_declared_paths`. This module is the next
//! layer on top of that raw file list:
//!
//! 1. [`build_plugin_workflow_inventory`] turns each discovered `.js`
//!    `ComponentPath` into a [`WorkflowInventoryEntry`] carrying its parsed
//!    `meta.name` and its namespaced scoped name `<plugin-name>:<meta.name>`
//!    (design line 418-422 — namespacing is by the script's OWN claimed
//!    `meta.name`, not by its filename, unlike commands/agents/skills).
//! 2. [`resolve_named_workflow`] is the general named-workflow resolver, in
//!    the oracle-aligned final order **saved > plugin > builtin** (design
//!    line 424): a project/user *saved* workflow is indexed by its own
//!    parsed `meta.name` (never filename), a *plugin* workflow by its
//!    namespaced FQN, and a *builtin* by its compiled-in name. A name that
//!    collides across tiers resolves to the first hit in that order.
//! 3. [`resolve_verified_handle`] / [`VerifiedWorkflowHandle`] model the
//!    OTHER half of design line 426-427: "Local App 产品调用始终使用 §8.1 的
//!    verified resolved handle，不经过这个可被 saved workflow shadow 的通用
//!    name resolver" — a Local App's own product call never goes through
//!    [`resolve_named_workflow`] at all, so a project/user saved workflow
//!    that happens to reuse the same scoped name can never redirect it. The
//!    full `LocalAppPluginBinding` / `WorkflowHandle` machinery design §8.1
//!    describes is a later-phase mobile composition-root concern (outside
//!    this crate); [`VerifiedWorkflowHandle`] here is the narrow `(name,
//!    path)` shape this task needs to prove that non-shadowing property.
//! 4. [`resolve_explicit_script_path`] documents the design's other carve-out
//!    (line 414-416): an explicit `scriptPath` (the `tool-workflow` crate's
//!    `WorkflowLaunchSpec::script_path` / `tasks`' `script_path` field) is
//!    NOT directory discovery and is therefore not subject to the `.js`-only
//!    gate at all — a caller pointing at a script directly is not being
//!    "discovered".
//!
//! ## A deliberate, documented gap
//!
//! [`extract_meta_name`] is a hand-rolled, best-effort scanner, not a real
//! parse. The authoritative implementation of "parse `meta.name` out of a
//! workflow script" already exists — `workflow::meta_string_value`, backed by
//! a real tree-sitter JS parse and exactly the contract `validate_meta`
//! (`workflow/src/lib.rs:377`) enforces — and this task's owned-files list
//! (`discovery.rs` / `lib.rs` / `workflow.rs`, no `Cargo.toml`) does not
//! extend to adding a `workflow` crate dependency to `plugin/Cargo.toml`
//! while another lane is concurrently editing this checkout. [`extract_meta_name`]
//! is scoped ONLY to building inventory display names / scoped names here; it
//! is never a substitute for `validate_meta` / `check_determinism`, which
//! still gate whether a script may actually run (see
//! `workflow/tests/plugin_workflow_scripts.rs`). Wiring `plugin` onto the
//! real `workflow::meta_string_value` is flagged as follow-up in this task's
//! report rather than done silently here.

use crate::discovery::glob_js;
use crate::manifest::ComponentPath;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Inventory
// ---------------------------------------------------------------------------

/// One discovered `.js` workflow script belonging to a plugin.
///
/// `meta_name` is `None` when [`extract_meta_name`] cannot find a plain
/// string-literal `name` field in the script's leading `export const meta =
/// { … }` block; `fqn` is then also `None` — a path-derived stand-in name
/// would let a script resolve under a name its own `meta` never claimed,
/// which the design's `<plugin-name>:<meta.name>` namespacing rule does not
/// describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowInventoryEntry {
    /// The contributing plugin's manifest `name`.
    pub plugin_name: String,
    /// Absolute (or install-dir-relative, matching `ComponentPath::path`'s
    /// own convention) path to the `.js` script.
    pub path: PathBuf,
    /// The script's own parsed `meta.name`, if recognizable.
    pub meta_name: Option<String>,
    /// `<plugin_name>:<meta_name>`, if `meta_name` was recognizable.
    pub fqn: Option<String>,
}

/// `<plugin_name>:<meta_name>` (design line 418-422).
#[must_use]
pub fn namespaced_workflow_name(plugin_name: &str, meta_name: &str) -> String {
    format!("{plugin_name}:{meta_name}")
}

/// Turn already-discovered `.js` [`ComponentPath`]s (e.g.
/// `PluginManifest::components.workflows`) into namespaced inventory
/// entries. Trusts its input list completely — the `.js`-only extension
/// gate is [`crate::discovery::glob_js`] / `resolve_workflow_declared_paths`'s
/// job, already applied before a path reaches here; this function does not
/// re-check extensions.
pub async fn build_plugin_workflow_inventory(
    plugin_name: &str,
    components: &[ComponentPath],
) -> Vec<WorkflowInventoryEntry> {
    let mut out = Vec::with_capacity(components.len());
    for cp in components {
        let meta_name = match tokio::fs::read_to_string(&cp.path).await {
            Ok(src) => extract_meta_name(&src),
            Err(_) => None,
        };
        let fqn = meta_name
            .as_deref()
            .map(|name| namespaced_workflow_name(plugin_name, name));
        out.push(WorkflowInventoryEntry {
            plugin_name: plugin_name.to_string(),
            path: cp.path.clone(),
            meta_name,
            fqn,
        });
    }
    out
}

/// Scan a plugin's workflow directory (the default `workflows/` layout — see
/// [`crate::discovery::glob_js`]) and build its namespaced inventory in one
/// call, for a caller that has a workflows directory in hand but no loaded
/// manifest.
///
/// ⚠️ This is a CONVENIENCE, not the production path, and the `.js`-only gate
/// is deliberately not characterized through it: the `workflow_dir_scan_rejects_*`
/// tests go through `discovery::load_plugin_from_path` →
/// `detect_components` → this module instead, because that is the list
/// `PluginManifest::components.workflows` actually carries, and because it
/// also covers the manifest-declared branch this function never reaches.
pub async fn scan_plugin_workflow_dir(
    plugin_name: &str,
    dir: &Path,
) -> Vec<WorkflowInventoryEntry> {
    let components = glob_js(dir).await;
    build_plugin_workflow_inventory(plugin_name, &components).await
}

/// Resolve an explicit `scriptPath` reference by reading the path directly.
///
/// This models the launch-time code path (`tool-workflow`'s
/// `WorkflowLaunchSpec::script_path` / `tasks`' `script_path` field), which
/// design line 414 calls out as NOT directory discovery: "显式 scriptPath 不
/// 属于目录 discovery，可继续读取 workflow engine 支持的脚本". A caller naming a
/// script by path is not being "discovered" — the `.js`-only gate exists to
/// keep near-miss files out of PASSIVE directory scans, and does not apply
/// here. Any readable file resolves, whatever its extension.
pub async fn resolve_explicit_script_path(path: &Path) -> Option<PathBuf> {
    let is_file = tokio::fs::metadata(path)
        .await
        .map(|meta| meta.is_file())
        .unwrap_or(false);
    if is_file {
        Some(path.to_path_buf())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Named resolver
// ---------------------------------------------------------------------------

/// A project/user "saved" workflow candidate for the named resolver's top
/// tier (design line 424-425): addressed by its OWN parsed `meta.name`,
/// never by filename.
///
/// This crate does not walk `.lingxi/workflows` itself — plugin workflow
/// discovery is scoped to a plugin's own directory
/// (`PluginComponents::workflows`'s doc comment on `manifest.rs` is explicit
/// that materialization/wiring beyond discovery is separate work), and
/// wiring the *project/user* saved-workflow directories `tool-workflow` owns
/// (`saved_workflow_dirs` in `tools/workflow/src/lib.rs`) into this resolver
/// is cross-crate integration outside this task's owned files. Callers
/// supply the already-resolved candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWorkflowCandidate {
    /// The candidate script's own parsed `meta.name`.
    pub meta_name: String,
    /// Where the script lives on disk.
    pub path: PathBuf,
}

/// A Local App's own verified workflow reference — the narrow `(scoped
/// name, path)` shape this task needs to prove design line 426-427's
/// non-shadowing property. The richer `LocalAppPluginBinding` /
/// `WorkflowHandle` design §8.1 describes (mobile composition root,
/// bytes-verified against a builtin descriptor the way
/// `BUILTIN_WORKFLOWS::is_local_app_build_script` verifies build-workflow
/// bytes today) is later-phase work outside this crate; this type is a
/// stand-in a future composition root can build such a handle down to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWorkflowHandle {
    /// The scoped name this handle answers for (e.g.
    /// `"lingxi-local-app:local-app-build"`).
    pub name: String,
    /// The verified script path.
    pub path: PathBuf,
}

impl VerifiedWorkflowHandle {
    /// Build a handle from a discovered plugin workflow.
    ///
    /// This is the only construction path this crate offers, and it takes a
    /// [`WorkflowInventoryEntry`] — something only plugin *discovery*
    /// produces. There is deliberately no `from_saved`: a project/user saved
    /// workflow cannot mint a handle, however it spells its `meta.name`.
    /// Returns `None` for an entry with no recognizable `meta.name` (and so
    /// no FQN), because a handle must answer for the namespaced name the
    /// script itself claims.
    #[must_use]
    pub fn from_plugin_inventory(entry: &WorkflowInventoryEntry) -> Option<Self> {
        Some(Self {
            name: entry.fqn.clone()?,
            path: entry.path.clone(),
        })
    }
}

/// Resolve a verified handle directly. This takes no `saved` /
/// `plugin_workflows` / `builtin_names` parameter at all — by construction,
/// nothing supplied to [`resolve_named_workflow`] can influence this
/// function's answer, which is exactly design line 426-427's contract: a
/// Local App's product call uses this, never the shadowable named resolver.
#[must_use]
pub fn resolve_verified_handle(handle: &VerifiedWorkflowHandle) -> PathBuf {
    handle.path.clone()
}

/// Where a resolved name came from, in the resolver's own precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedWorkflowSource {
    /// A project/user saved workflow, matched on its own parsed `meta.name`.
    Saved,
    /// A namespaced plugin-contributed workflow (`<plugin>:<meta.name>`).
    NamespacedPlugin,
    /// A compiled-in builtin workflow.
    Builtin,
}

/// One [`resolve_named_workflow`] hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkflow {
    /// Which tier answered.
    pub source: ResolvedWorkflowSource,
    /// The resolved script path — `None` for [`ResolvedWorkflowSource::Builtin`],
    /// which is compiled-in and has no on-disk path.
    pub path: Option<PathBuf>,
}

/// The general named-workflow resolver (design line 424): **saved > plugin >
/// builtin**. A name that collides across tiers resolves to the first hit in
/// that order.
///
/// ⚠️ Shadowable BY DESIGN — a project/user saved workflow can claim any
/// name, including one a plugin or builtin already uses; that is the whole
/// reason design line 426-427 says a Local App's own product call must
/// bypass this function and use [`resolve_verified_handle`] instead.
#[must_use]
pub fn resolve_named_workflow(
    query: &str,
    saved: &[SavedWorkflowCandidate],
    plugin_workflows: &[WorkflowInventoryEntry],
    builtin_names: &BTreeSet<String>,
) -> Option<ResolvedWorkflow> {
    if let Some(hit) = saved.iter().find(|candidate| candidate.meta_name == query) {
        return Some(ResolvedWorkflow {
            source: ResolvedWorkflowSource::Saved,
            path: Some(hit.path.clone()),
        });
    }
    if let Some(hit) = plugin_workflows
        .iter()
        .find(|entry| entry.fqn.as_deref() == Some(query))
    {
        return Some(ResolvedWorkflow {
            source: ResolvedWorkflowSource::NamespacedPlugin,
            path: Some(hit.path.clone()),
        });
    }
    if builtin_names.contains(query) {
        return Some(ResolvedWorkflow {
            source: ResolvedWorkflowSource::Builtin,
            path: None,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Task script copy + digest (spec §19.1 — "task copy, digest, pause/resume")
// ---------------------------------------------------------------------------
//
// The property (task write-up, verbatim): a workflow task that pauses and
// later resumes must run the SAME script bytes it started with. If the
// plugin is updated, disabled, or its file edited between pause and resume,
// resuming against the new bytes silently changes what the task does
// mid-flight — the journal replays prior `agent()` results while the JS body
// re-runs, so a changed body reads stale results as its own. This is exactly
// the reasoning `workflow::check_determinism`'s doc comment gives for why
// resume needs determinism at all (`workflow/src/lib.rs:614-621`): journal
// replay assumes the re-run body is the SAME body. A drifted script breaks
// that assumption every bit as badly as `Date.now()` does.
//
// So: a launched task takes a COPY of the script bytes and a DIGEST of them
// at launch ([`TaskScriptCopy`]), and resume must verify the digest against
// whatever bytes it is about to re-run before proceeding
// ([`verify_resume_digest`]).
//
// ## What this crate can and cannot deliver
//
// [`script_digest`], [`TaskScriptCopy`], and [`verify_resume_digest`] are the
// full reachable mechanism from `plugin/` alone: compute a digest, capture a
// copy+digest pair at launch, and decide go/no-go for a resume given the
// launch-time copy and the script bytes resume is about to run. All three are
// exercised end-to-end below, through the SAME production discovery path
// (`load_plugin_from_path` → `detect_components` → `build_plugin_workflow_inventory`)
// the rest of this module's tests use, with a real on-disk file edit standing
// in for "the plugin's script changed between pause and resume".
//
// What this crate does **not** and structurally **cannot** deliver: wiring
// [`verify_resume_digest`] into an actual task-resume call site. That call
// site does not exist inside `plugin/` — resume is orchestrated by
// `tools/workflow`'s `Workflow` tool (`tools/workflow/src/lib.rs`, the
// errorCode-3 "still-running resume target" gate and the `scriptPath`
// re-read branch just above it) together with the task row `tasks::state::LocalWorkflowTaskState`
// persists (`tasks/src/state.rs:296-386`, explicitly NOT owned by this task).
// Today that row has a `script: String` copy (already "carried for resume",
// per its own doc comment) and a `script_path: Option<String>`, but **no
// digest field** — so there is nowhere on the task row to persist the
// launch-time digest this property needs, and no reachable call site to
// invoke the check against it. Closing that gap needs, in a follow-up whose
// owned files include `tasks/src/state.rs` and `tools/workflow/src/lib.rs`:
//
// 1. A new field on `LocalWorkflowTaskState`, e.g.
//    `#[serde(default)] pub launch_script_digest: Option<String>`, populated
//    at launch from `plugin::workflow::script_digest(&resolved_script)`
//    (mirroring how `script`/`script_path` are already populated there).
// 2. A call in `tools/workflow`'s resume handling — right where it already
//    re-reads the file with `std::fs::read(&resolved_path)` into
//    `resolved_script` (`tools/workflow/src/lib.rs`, the `scriptPath` arm of
//    the errorCode-1 resolution) and before its errorCode-3 still-running
//    check — to `plugin::workflow::verify_resume_digest` against that
//    persisted digest, surfacing a mismatch as a new `ValidationError` in the
//    same byte-exact-message style as errorCode 2/4.
// 3. That call site does not compile today for a reason beyond ownership:
//    `tools/workflow/Cargo.toml` does not depend on `plugin` at all (only
//    `apps/engine-desktop` and `apps/cli` do). The follow-up must add that
//    dependency. It is acyclic — `plugin`'s own dependency closure
//    (branding/protocol/traits/tool-api/hooks/mcp/agent/skill-api/command-api/
//    outputstyles/lsp/secret) does not reach `tools/workflow`.
//
// That the drift is real and not hypothetical is visible in the launch text
// `tools/workflow` already emits: it tells the model, verbatim, "To resume
// after editing the script: Workflow({scriptPath: …, resumeFromRunId: …})".
// The resume then re-reads the file — the CURRENT bytes — while the journal
// still replays the OLD run's `agent()` results. This module is the check
// that path is missing.
//
// Neither of those two call sites is reachable from this task's owned files
// (`plugin/src/workflow.rs` alone), so the test below pins the strongest
// thing that genuinely IS reachable — the digest-copy-verify mechanism
// itself, driven through real discovery and a real file mutation — rather
// than a weaker "two hashes differ" tautology wearing the "refuses resume"
// name, and rather than a test that only LOOKS wired by calling through a
// resume path that does not exist here.

/// Lowercase-hex SHA-256 of a script's bytes, taken as the DIGEST half of a
/// launch-time [`TaskScriptCopy`]. Delegates to [`crate::plugin_source_sha256`]
/// (the public re-export of `mcpb::sha256_hex`, which is a private module) —
/// the same SHA-256 integrity check this crate already uses to verify a
/// `.mcpb` bundle's contents — rather than hashing independently, so there is
/// exactly one SHA-256 code path in this crate to keep correct.
///
/// SHA-256 specifically, and not merely "some fingerprint that changes when
/// the script changes", is the load-bearing part: the drift this gate exists
/// to catch is frequently SAME-LENGTH (`'create'` → `'delete'`,
/// `maxSteps: 3` → `maxSteps: 8`), so a length- or size-derived stand-in would
/// wave exactly the realistic edits through. `script_digest_is_sha256_of_the_scripts_bytes`
/// pins it to published SHA-256 test vectors for that reason.
#[must_use]
pub fn script_digest(script: &str) -> String {
    crate::mcpb::sha256_hex(script.as_bytes())
}

/// The launch-time COPY a workflow task carries forward, paired with its
/// digest, taken together at the same instant. `script` is the same shape
/// `tasks::LocalWorkflowTaskState::script` already persists ("carried for
/// resume"); `digest` is what a resume call must check before trusting that
/// carried copy — or before trusting a re-read `scriptPath` — is still the
/// script the task actually launched with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskScriptCopy {
    /// The script bytes exactly as they existed at launch.
    pub script: String,
    /// [`script_digest`] of `script`, taken at the same instant — frozen at
    /// capture time, independent of anything that happens to `script` on disk
    /// afterward.
    pub digest: String,
}

impl TaskScriptCopy {
    /// Take a copy+digest of already-in-hand script text (e.g. an inline
    /// `script` input, or bytes a caller already resolved from a path).
    #[must_use]
    pub fn at_launch(script: &str) -> Self {
        Self {
            script: script.to_string(),
            digest: script_digest(script),
        }
    }

    /// Take a copy+digest by reading a script off disk right now — the
    /// launch-time capture for a plugin-discovered workflow. Uses the same
    /// `tokio::fs::read_to_string` this module's own
    /// [`build_plugin_workflow_inventory`] already reads scripts through, so
    /// a caller capturing a copy this way is reading exactly the bytes
    /// discovery itself saw.
    ///
    /// # Errors
    /// Propagates the underlying [`tokio::fs::read_to_string`] I/O error.
    pub async fn read_at_launch(path: &Path) -> std::io::Result<Self> {
        let script = tokio::fs::read_to_string(path).await?;
        Ok(Self::at_launch(&script))
    }
}

/// A resume was refused because the script it is about to run no longer
/// matches the digest captured at launch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "workflow script changed since launch (expected digest {expected}, found {actual}); \
     refusing resume so a paused task never re-runs a different script body"
)]
pub struct ScriptDigestMismatch {
    /// The digest captured in [`TaskScriptCopy::digest`] at launch.
    pub expected: String,
    /// [`script_digest`] of the bytes resume was about to run.
    pub actual: String,
}

/// The resume gate this task exists to build: verify that `current_script`
/// (the bytes a resume is about to re-run — a re-read `scriptPath`, or
/// whatever else a caller resolves) still matches the digest captured in
/// `copy` at launch. `Ok(())` means resume may proceed; `Err` means the
/// script drifted between pause and resume and resume must be refused (see
/// this section's module-level doc for why: a changed body would read stale
/// journaled `agent()` results as its own).
///
/// # Errors
/// Returns [`ScriptDigestMismatch`] when `current_script`'s digest does not
/// match `copy.digest`.
pub fn verify_resume_digest(
    copy: &TaskScriptCopy,
    current_script: &str,
) -> Result<(), ScriptDigestMismatch> {
    let actual = script_digest(current_script);
    if actual == copy.digest {
        Ok(())
    } else {
        Err(ScriptDigestMismatch {
            expected: copy.digest.clone(),
            actual,
        })
    }
}

// ---------------------------------------------------------------------------
// `meta.name` extraction (best-effort — see module doc's "deliberate gap")
// ---------------------------------------------------------------------------

/// Extract `meta.name`'s plain string-literal value from a workflow script.
///
/// Recognizes `export const meta = { … name: 'literal' … }` /
/// `name: "literal"` (the shape every checked-in plugin workflow script
/// under `plugins/lingxi-local-app/workflows/*.js` uses) by finding the
/// balanced `{ … }` object literal that follows the script's first
/// `export const meta` occurrence **in comment-stripped source**, then
/// scanning that object's own text for a `name` key followed by `:` and a
/// quoted string, returning its (minimally unescaped) contents. Returns
/// `None` when no such shape is found — a computed/templated/missing `name`
/// yields no scoped name at all rather than a guessed one.
///
/// ## Why the comment strip is load-bearing
///
/// Every checked-in fixture opens with a long `//` header, and
/// `local-app-build.js:19` *mentions* `` `export const meta` `` in that
/// header, 42 lines above the real declaration at `:61`. Scanning raw source
/// therefore anchors on the COMMENT, and then takes the next `{` it sees —
/// which happens to be the real object today only because no brace appears in
/// between. One `{ … }` example added to that header (the file already writes
/// `{ operation: … }` shapes just below, at `:69`) would silently make this
/// return `None`, dropping the script's `meta_name` and its FQN and making it
/// unaddressable by [`resolve_named_workflow`]'s plugin tier — with no error
/// anywhere. Comments (and apostrophes inside them, which would otherwise
/// open a phantom string in [`balanced_braces`]) are removed first so the
/// anchor is the real declaration.
///
/// This is NOT `workflow::validate_meta`'s tree-sitter parse and does not
/// enforce meta-object purity, first-statement position, or non-emptiness;
/// see the module doc for why a real dependency on the `workflow` crate is
/// not wired in here.
#[must_use]
pub fn extract_meta_name(script: &str) -> Option<String> {
    let src = strip_js_comments(script);
    let meta_kw = src.find("export const meta")?;
    let after = &src[meta_kw..];
    let brace_at = after.find('{')?;
    let obj = balanced_braces(&after[brace_at..])?;
    quoted_field_value(obj, "name")
}

/// Replace every `//…` line comment and `/*…*/` block comment with an equal
/// run of spaces, leaving string and template-literal contents untouched (so
/// a `//` inside `'https://x'` survives) and preserving byte offsets.
///
/// A `/` that actually opens a JS regex literal whose body starts with `/` or
/// `*` would be misread, but `//` is never a valid regex start (it is a
/// comment) and `/*` cannot begin one either, so no real script is affected.
fn strip_js_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    // Blanking writes ASCII `0x20` only over bytes inside an ASCII-delimited
    // comment run, so the result is still valid UTF-8 — a multi-byte
    // character's continuation bytes are only ever reached as part of such a
    // run, and then every byte of it is blanked together.
    let mut buf = bytes.to_vec();
    let mut i = 0usize;
    let mut in_str: Option<u8> = None;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' | b'`' => {
                in_str = Some(c);
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    buf[i] = b' ';
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = src[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |rel| i + 2 + rel + 2);
                while i < end {
                    if bytes[i] != b'\n' {
                        buf[i] = b' ';
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(buf).expect("comment blanking only overwrites ASCII comment bytes")
}

/// Return the substring of `s` (which must start with `{`) up to and
/// including the matching closing `}`, tracking string/template-literal
/// state (with backslash-escape handling) so a brace inside a quoted value
/// never miscounts depth.
fn balanced_braces(s: &str) -> Option<&str> {
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if let Some(quote) = in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                in_str = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => in_str = Some(c),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..i + c.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Within an object-literal source `obj`, find the first `field` token at a
/// word boundary (not e.g. the tail of `meta.name` — the preceding character
/// must not be alphanumeric/`_`/`.`/`$`) immediately followed by an optional
/// closing quote (for a quoted key like `"name"`), whitespace, `:`,
/// whitespace, and a single- or double-quoted string; returns that string's
/// contents with a minimal backslash-escape pass. Skips past any `field`
/// occurrence that isn't shaped like a key (e.g. one appearing inside
/// another field's string value) and keeps scanning.
fn quoted_field_value(obj: &str, field: &str) -> Option<String> {
    let mut search_from = 0usize;
    loop {
        let rel = obj.get(search_from..)?.find(field)?;
        let pos = search_from + rel;
        let end = pos + field.len();
        search_from = end;

        let boundary_ok = obj[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '$'));
        if !boundary_ok {
            continue;
        }

        let mut rest = &obj[end..];
        if let Some(stripped) = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\'')) {
            rest = stripped;
        }
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else {
            continue;
        };
        let value_src = &rest[quote.len_utf8()..];
        let mut out = String::new();
        let mut escaped = false;
        for c in value_src.chars() {
            if escaped {
                out.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == quote {
                return Some(out);
            }
            out.push(c);
        }
        return None; // unterminated string — malformed script.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::load_plugin_from_path;
    use std::fs;

    // -- helpers ----------------------------------------------------------

    /// A meta block whose `name` is `name`, in the exact shape every
    /// checked-in plugin workflow script uses.
    fn script(name: &str) -> String {
        format!("export const meta = {{\n  name: '{name}',\n  description: 'd',\n}};\n")
    }

    /// Write a plugin root with the given manifest body and return its path.
    fn plugin_root(tmp: &Path, manifest_json: &str) -> PathBuf {
        let root = tmp.to_path_buf();
        let manifest_dir = root.join(branding::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(manifest_dir.join("plugin.json"), manifest_json).unwrap();
        root
    }

    /// The PRODUCTION path: real manifest load → `detect_components`'s
    /// `.js`-only gate → inventory. Deliberately not `scan_plugin_workflow_dir`
    /// — a gate proven only on a parallel scan helper says nothing about the
    /// list `PluginManifest::components.workflows` actually carries.
    async fn discovered_inventory(plugin_dir: &Path) -> Vec<WorkflowInventoryEntry> {
        let (_id, manifest) = load_plugin_from_path(plugin_dir)
            .await
            .expect("fixture plugin must load");
        build_plugin_workflow_inventory(&manifest.name, &manifest.components.workflows).await
    }

    fn file_names(inventory: &[WorkflowInventoryEntry]) -> Vec<String> {
        inventory
            .iter()
            .map(|e| e.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect()
    }

    // -- extract_meta_name -------------------------------------------------

    #[test]
    fn extract_meta_name_matches_the_three_checked_in_plugin_workflow_scripts() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/lingxi-local-app/workflows");
        let expected = [
            ("local-app-build.js", "local-app-build"),
            ("local-app-mcp-authoring.js", "local-app-mcp-authoring"),
            ("local-app-use-test.js", "local-app-use-test"),
        ];
        for (file, want) in expected {
            let path = dir.join(file);
            let src = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
            assert_eq!(
                extract_meta_name(&src).as_deref(),
                Some(want),
                "meta.name extraction mismatch for {file}"
            );
        }
    }

    /// Regression pin for the defect the review found: `local-app-build.js:19`
    /// MENTIONS `export const meta` inside its `//` header, 42 lines above the
    /// real declaration at `:61`, and the header writes `{ operation: … }`
    /// example shapes just below at `:69`. Anchoring on raw source therefore
    /// latches onto the comment and then onto whatever `{` comes next; the
    /// fixture test above passed only because no brace happens to sit between
    /// those two lines today. Each case below is a header that DOES put a
    /// brace (or a stray apostrophe, which would open a phantom string in
    /// `balanced_braces`) in the way — before the fix all four returned
    /// `None`, silently costing the script its `meta_name` and its FQN.
    #[test]
    fn extract_meta_name_survives_a_header_comment_that_mentions_the_meta_declaration() {
        let cases = [
            (
                "line comment naming the declaration, then a braced example",
                concat!(
                    "// `export const meta` must be the first statement.\n",
                    "// Args look like { operation: 'create' }.\n",
                    "export const meta = {\n  name: 'local-app-build',\n};\n",
                ),
            ),
            (
                "block comment naming the declaration, then a braced example",
                concat!(
                    "/* export const meta is the first statement.\n",
                    "   Args: { operation: 'create' } */\n",
                    "export const meta = {\n  name: 'local-app-build',\n};\n",
                ),
            ),
            (
                "apostrophe in a comment inside the meta object",
                concat!(
                    "export const meta = {\n",
                    "  // the engine's required first statement\n",
                    "  name: 'local-app-build',\n};\n",
                ),
            ),
            (
                "brace in a comment inside the meta object, before name",
                concat!(
                    "export const meta = {\n",
                    "  // shape: { name, description }\n",
                    "  name: 'local-app-build',\n};\n",
                ),
            ),
        ];
        for (label, src) in cases {
            assert_eq!(
                extract_meta_name(src).as_deref(),
                Some("local-app-build"),
                "comment-shaped header defeated extraction: {label}"
            );
        }
    }

    #[test]
    fn extract_meta_name_ignores_meta_name_property_access_outside_the_object() {
        // Positive control: a script whose object literal has NO `name` key
        // at all, but whose body (outside the object) uses `meta.name` in a
        // template string exactly like the real fixtures do. If the scanner
        // were naively grepping the whole script for the token `name` it
        // would find this and misreport; scoping the scan to the balanced
        // `{ … }` object must exclude it.
        let src = concat!(
            "export const meta = {\n",
            "  description: 'no name field here',\n",
            "};\n",
            "console.log(`${meta.name} ran`);\n",
        );
        assert_eq!(extract_meta_name(src), None);
    }

    #[test]
    fn strip_js_comments_leaves_comment_markers_inside_string_literals_alone() {
        // Positive control for the stripper itself: without this, a stripper
        // that blanked `//` anywhere would silently truncate a `name` whose
        // value contains a URL, and every assertion above would still pass.
        let src =
            "export const meta = { name: 'https://x/y', description: '/* not a comment */' };";
        assert_eq!(extract_meta_name(src).as_deref(), Some("https://x/y"));
    }

    // -- the required extension-gate tests, on the production path ---------

    #[tokio::test]
    async fn workflow_dir_scan_rejects_mjs_near_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"demo-plugin"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.js"), script("real-one")).unwrap();
        fs::write(dir.join("near.mjs"), script("near-miss")).unwrap();

        let inventory = discovered_inventory(&root).await;

        // Positive control: the near-miss file really is on disk, sitting
        // right next to the real one — an empty inventory would prove
        // nothing about the gate without this. And the near-miss carries a
        // perfectly valid `meta.name`, so nothing but the EXTENSION can be
        // what kept it out.
        assert!(dir.join("near.mjs").is_file());
        assert_eq!(
            extract_meta_name(&fs::read_to_string(dir.join("near.mjs")).unwrap()).as_deref(),
            Some("near-miss")
        );
        assert_eq!(
            file_names(&inventory),
            vec!["real.js"],
            "expected exactly the .js entry, got {inventory:?}"
        );
        assert_eq!(inventory[0].meta_name.as_deref(), Some("real-one"));
        assert_eq!(inventory[0].fqn.as_deref(), Some("demo-plugin:real-one"));
    }

    #[tokio::test]
    async fn workflow_dir_scan_rejects_cjs_and_ts_near_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"demo-plugin"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.js"), script("real-two")).unwrap();
        for near in ["near.cjs", "near.ts", "near"] {
            fs::write(dir.join(near), script("near-miss")).unwrap();
        }

        let inventory = discovered_inventory(&root).await;

        // Positive controls, same reasoning as the `.mjs` test above.
        for near in ["near.cjs", "near.ts", "near"] {
            assert!(dir.join(near).is_file(), "{near} must exist on disk");
        }
        assert_eq!(
            file_names(&inventory),
            vec!["real.js"],
            "expected exactly the .js entry, got {inventory:?}"
        );
        assert_eq!(inventory[0].fqn.as_deref(), Some("demo-plugin:real-two"));
    }

    /// The gate has a SECOND construction path: a manifest `workflows`
    /// declaration goes through `resolve_workflow_declared_paths`, not
    /// `glob_js`'s default-directory branch. Widening only that branch would
    /// leave both tests above green.
    #[tokio::test]
    async fn manifest_declared_workflow_paths_reject_the_same_near_misses() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(
            tmp.path(),
            r#"{"name":"demo-plugin","workflows":["./scripts/ok.js","./scripts/near.mjs","./more"]}"#,
        );
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("more")).unwrap();
        fs::write(root.join("scripts/ok.js"), script("declared-file")).unwrap();
        fs::write(root.join("scripts/near.mjs"), script("declared-near")).unwrap();
        fs::write(root.join("more/dir.js"), script("declared-dir")).unwrap();
        fs::write(root.join("more/dir.cjs"), script("declared-dir-near")).unwrap();
        fs::write(root.join("more/dir.ts"), script("declared-dir-near-ts")).unwrap();

        let inventory = discovered_inventory(&root).await;

        assert!(root.join("scripts/near.mjs").is_file());
        assert!(root.join("more/dir.cjs").is_file());
        let mut names = file_names(&inventory);
        names.sort();
        assert_eq!(
            names,
            vec!["dir.js", "ok.js"],
            "declared-path branch let a near-miss through: {inventory:?}"
        );
        let fqns: BTreeSet<_> = inventory.iter().filter_map(|e| e.fqn.clone()).collect();
        assert_eq!(
            fqns,
            BTreeSet::from([
                "demo-plugin:declared-dir".to_string(),
                "demo-plugin:declared-file".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn explicit_script_path_is_not_subject_to_the_extension_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"demo-plugin"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.js"), script("real-three")).unwrap();
        let mjs_path = dir.join("bundled.mjs");
        fs::write(&mjs_path, script("bundled")).unwrap();

        // Directory discovery: the .mjs near-miss is excluded.
        let inventory = discovered_inventory(&root).await;
        assert_eq!(file_names(&inventory), vec!["real.js"]);

        // The SAME file, addressed as an explicit scriptPath — a different
        // code path per §19.1 — resolves regardless of its extension.
        assert_eq!(
            resolve_explicit_script_path(&mjs_path).await,
            Some(mjs_path.clone())
        );
        // …and so do the other near-miss extensions, so this is a statement
        // about the ABSENCE of a gate, not about `.mjs` in particular.
        for near in ["other.cjs", "other.ts", "other"] {
            let p = dir.join(near);
            fs::write(&p, script("x")).unwrap();
            assert_eq!(resolve_explicit_script_path(&p).await, Some(p.clone()));
        }

        // Negative control: a path that genuinely does not exist still
        // fails, so this isn't a function that always returns `Some`.
        assert_eq!(
            resolve_explicit_script_path(&dir.join("does-not-exist.mjs")).await,
            None
        );
        // Second negative control: a DIRECTORY is not a script, so `Some` is
        // not simply "the path string was non-empty".
        assert_eq!(resolve_explicit_script_path(&dir).await, None);
    }

    // -- resolver ----------------------------------------------------------

    #[test]
    fn resolver_orders_saved_meta_name_over_namespaced_plugin_over_builtin() {
        // The plugin tier is addressed by its NAMESPACED name; `meta_name`
        // and `fqn` are deliberately DIFFERENT strings here so a resolver
        // that matched `meta_name` instead of `fqn` — letting a plugin
        // workflow answer to a bare, unnamespaced name — cannot pass.
        const META: &str = "collision";
        let fqn = namespaced_workflow_name("acme", META);
        assert_eq!(fqn, "acme:collision");

        let plugin_workflows = vec![WorkflowInventoryEntry {
            plugin_name: "acme".to_string(),
            path: PathBuf::from("/plugin/collision.js"),
            meta_name: Some(META.to_string()),
            fqn: Some(fqn.clone()),
        }];
        // Nothing forbids a colon in a SAVED workflow's own `meta.name`, so a
        // saved script can claim a plugin's namespaced name outright — which
        // is precisely the shadow the design warns about.
        let saved = vec![SavedWorkflowCandidate {
            meta_name: fqn.clone(),
            path: PathBuf::from("/saved/collision.js"),
        }];
        let builtin_names = BTreeSet::from([fqn.clone()]);

        // All three tiers claim the same name: saved must win.
        let resolved =
            resolve_named_workflow(&fqn, &saved, &plugin_workflows, &builtin_names).unwrap();
        assert_eq!(resolved.source, ResolvedWorkflowSource::Saved);
        assert_eq!(
            resolved.path.as_deref(),
            Some(Path::new("/saved/collision.js"))
        );

        // Inversion control #1: drop the saved tier. The SAME plugin/builtin
        // data must now resolve to the plugin tier — proving the previous
        // result was the saved tier's PRIORITY, not an artifact of insertion
        // order or of only one tier ever being populated.
        let resolved =
            resolve_named_workflow(&fqn, &[], &plugin_workflows, &builtin_names).unwrap();
        assert_eq!(resolved.source, ResolvedWorkflowSource::NamespacedPlugin);
        assert_eq!(
            resolved.path.as_deref(),
            Some(Path::new("/plugin/collision.js"))
        );

        // Inversion control #2: drop plugin too. Only builtin remains.
        let resolved = resolve_named_workflow(&fqn, &[], &[], &builtin_names).unwrap();
        assert_eq!(resolved.source, ResolvedWorkflowSource::Builtin);
        assert_eq!(resolved.path, None);

        // Inversion control #3: drop everything. No resolution at all.
        assert_eq!(
            resolve_named_workflow(&fqn, &[], &[], &BTreeSet::new()),
            None
        );

        // Negative control on the plugin tier's KEY: the bare `meta.name` is
        // not an address. Without this, `meta_name == fqn` in the fixture
        // would let a bare-name resolver pass every assertion above.
        assert_eq!(
            resolve_named_workflow(META, &[], &plugin_workflows, &BTreeSet::new()),
            None,
            "a plugin workflow must not be addressable by its unnamespaced meta.name"
        );
    }

    #[tokio::test]
    async fn a_same_named_saved_workflow_does_not_shadow_the_verified_handle() {
        // The handle is built from PRODUCTION DISCOVERY, not from a literal
        // the assertion then reads back: `verified.path` is a tempdir path
        // this test never types out, so a getter that returned a constant —
        // or a handle sourced from anywhere but plugin discovery — fails.
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"lingxi-local-app"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("local-app-build.js"), script("local-app-build")).unwrap();

        let inventory = discovered_inventory(&root).await;
        assert_eq!(file_names(&inventory), vec!["local-app-build.js"]);
        let verified = VerifiedWorkflowHandle::from_plugin_inventory(&inventory[0])
            .expect("a discovered workflow with a meta.name yields a handle");
        assert_eq!(verified.name, "lingxi-local-app:local-app-build");
        assert_eq!(verified.path, dir.join("local-app-build.js"));

        // An impostor saved workflow claims the exact same scoped name.
        let saved = vec![SavedWorkflowCandidate {
            meta_name: verified.name.clone(),
            path: tmp.path().join("impostor.js"),
        }];

        // The Local App product call — handle-direct — still lands on the
        // discovered plugin script.
        assert_eq!(
            resolve_verified_handle(&verified),
            dir.join("local-app-build.js")
        );
        assert_ne!(resolve_verified_handle(&verified), saved[0].path);

        // Positive control: the shadow is REAL in the general resolver, and
        // it beats THIS EXACT plugin entry — otherwise "not shadowed" would
        // be a vacuous claim about a resolver that was never at risk. This
        // is exactly why the product call must not use it.
        let shadowed =
            resolve_named_workflow(&verified.name, &saved, &inventory, &BTreeSet::new()).unwrap();
        assert_eq!(shadowed.source, ResolvedWorkflowSource::Saved);
        assert_eq!(shadowed.path.as_deref(), Some(saved[0].path.as_path()));
        // …and with the impostor removed the same call returns the plugin
        // entry, so the tier really was populated and really did lose.
        let unshadowed =
            resolve_named_workflow(&verified.name, &[], &inventory, &BTreeSet::new()).unwrap();
        assert_eq!(unshadowed.source, ResolvedWorkflowSource::NamespacedPlugin);
        assert_eq!(unshadowed.path.as_deref(), Some(verified.path.as_path()));
    }

    /// `scan_plugin_workflow_dir` is a convenience seam with no production
    /// caller, so pin it DIFFERENTIALLY against the production path on the
    /// same tree rather than leaving it untested and free to drift: if the
    /// helper ever stopped applying the same `.js`-only gate, or built a
    /// different FQN, the two lists would diverge here.
    #[tokio::test]
    async fn the_directory_scan_helper_agrees_with_production_discovery() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"demo-plugin"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("a.js"), script("alpha")).unwrap();
        fs::write(dir.join("nested/b.js"), script("beta")).unwrap();
        fs::write(dir.join("c.mjs"), script("gamma")).unwrap();

        let production = discovered_inventory(&root).await;
        let helper = scan_plugin_workflow_dir("demo-plugin", &dir).await;
        // Positive control: neither list is empty, so equality is not the
        // trivial `[] == []`.
        assert_eq!(file_names(&production), vec!["a.js", "b.js"]);
        assert_eq!(production, helper);
    }

    /// A discovered script whose `meta.name` cannot be parsed gets no FQN,
    /// and therefore no handle — rather than a filename-derived stand-in name
    /// it never claimed.
    #[tokio::test]
    async fn a_workflow_without_a_parsable_meta_name_yields_no_fqn_and_no_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"demo-plugin"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("anon.js"),
            "export const meta = { description: 'd' };\n",
        )
        .unwrap();

        let inventory = discovered_inventory(&root).await;
        assert_eq!(file_names(&inventory), vec!["anon.js"]);
        assert_eq!(inventory[0].meta_name, None);
        assert_eq!(inventory[0].fqn, None);
        assert!(VerifiedWorkflowHandle::from_plugin_inventory(&inventory[0]).is_none());
        // It is also unreachable through the plugin tier under the name a
        // filename-derived fallback would have given it.
        assert_eq!(
            resolve_named_workflow("demo-plugin:anon", &[], &inventory, &BTreeSet::new()),
            None
        );
    }

    // -- task script copy + digest (pause/resume) --------------------------

    /// Pins `script_digest` to SHA-256 by KNOWN ANSWER, against published
    /// vectors — not by "the digest changed when the script changed".
    ///
    /// This test exists because of a hole the review found and reproduced:
    /// with `script_digest` replaced by `format!("{:x}", script.len())`, ALL
    /// 14 tests in this module passed, including
    /// `a_changed_script_digest_refuses_resume`. Every drift the fixtures
    /// staged happened to change the script's LENGTH, so the suite pinned
    /// "some value moved", not "the bytes are digested". A length-derived
    /// stand-in waves through exactly the edits this gate exists to catch —
    /// `'create'` → `'delete'`, `maxSteps: 3` → `maxSteps: 8` — because those
    /// are same-length. Known answers cannot be satisfied by any function
    /// other than SHA-256-hex.
    #[test]
    fn script_digest_is_sha256_of_the_scripts_bytes() {
        // Published SHA-256 vectors, independently confirmed with
        // `printf '…' | shasum -a 256` rather than quoted from memory.
        assert_eq!(
            script_digest(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "script_digest(\"\") must be the SHA-256 of the empty input"
        );
        assert_eq!(
            script_digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "script_digest(\"abc\") must be the SHA-256 of \"abc\""
        );
        // A realistic workflow script, digested by the same external tool:
        //   printf "export const meta = {\n  name: 'a',\n  description: \
        //   'd',\n};\n" | shasum -a 256
        // The bytes are spelled out here rather than taken from `script("a")`
        // so the vector's provenance is unambiguous — and then checked to be
        // exactly what the helper produces, so the helper cannot drift away
        // from the pinned vector unnoticed.
        const FIXTURE: &str = "export const meta = {\n  name: 'a',\n  description: 'd',\n};\n";
        assert_eq!(script("a"), FIXTURE, "the `script` helper's shape changed");
        assert_eq!(
            script_digest(FIXTURE),
            "458d2173b3f9a7f70388d44b38e5a1c32d96ef3cd88935f74908c6485ab8f98d",
            "script_digest of the fixture script must be its SHA-256"
        );

        // Shape: 64 lowercase hex characters, always.
        let d = script_digest(&script("some-workflow"));
        assert_eq!(d.len(), 64, "expected 64 hex chars, got {d:?}");
        assert!(
            d.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "expected lowercase hex, got {d:?}"
        );

        // The property the length plant broke, stated directly: two scripts of
        // the SAME length and different bytes must digest differently.
        let a = "export const meta = {\n  name: 'create',\n};\n";
        let b = "export const meta = {\n  name: 'delete',\n};\n";
        assert_eq!(a.len(), b.len(), "fixture setup error: lengths must match");
        assert_ne!(a, b);
        assert_ne!(
            script_digest(a),
            script_digest(b),
            "a same-length edit must still change the digest"
        );
    }

    /// Pins [`TaskScriptCopy`] as a launch-time snapshot: the copy answers for
    /// the bytes that existed when it was taken, and a later edit to the file
    /// it came from does not retroactively change it.
    ///
    /// Honest scoping: that `copy.script` cannot mutate when the SOURCE
    /// mutates is structural in Rust (the field is an owned `String`), so
    /// asserting it proves nothing. The assertions that can actually fail are
    /// the ones about the DIGEST's binding to the captured bytes, and about
    /// [`TaskScriptCopy::read_at_launch`] having snapshotted disk contents at
    /// read time rather than answering for whatever the file holds later.
    /// Uses non-ASCII content so this is a statement about the copy's BYTES,
    /// not merely its parsed ASCII `meta.name`.
    #[tokio::test]
    async fn task_script_copy_snapshots_the_bytes_it_was_taken_from() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("snapshot.js");
        let original = "export const meta = {\n  name: 'ünïcode-résumé',\n};\n// 🚀\n";
        fs::write(&path, original).unwrap();

        let copy = TaskScriptCopy::read_at_launch(&path)
            .await
            .expect("fixture script must be readable");
        assert_eq!(copy.script, original);
        assert_eq!(copy.digest, script_digest(original));
        // …and `at_launch` on the same bytes agrees, so the two constructors
        // are not two different capture semantics.
        assert_eq!(copy, TaskScriptCopy::at_launch(original));

        // Edit the FILE the copy was read from, keeping the byte length
        // identical, so a length-derived digest cannot pass this.
        let edited = "export const meta = {\n  name: 'ünïcode-résumé',\n};\n// 🛰\n";
        assert_eq!(edited.len(), original.len(), "fixture: lengths must match");
        assert_ne!(edited, original);
        fs::write(&path, edited).unwrap();

        // The copy still answers for the ORIGINAL bytes.
        assert_eq!(copy.script, original);
        assert_eq!(
            copy.digest,
            script_digest(original),
            "the copy's digest must still answer for the ORIGINAL bytes"
        );
        assert_ne!(
            copy.digest,
            script_digest(edited),
            "a same-length on-disk edit must not collide with the captured digest"
        );
        // Positive control: a fresh read of the same path really does see the
        // new bytes, so the assertions above are not passing because the write
        // silently failed.
        let reread = TaskScriptCopy::read_at_launch(&path).await.unwrap();
        assert_eq!(reread.script, edited);
        assert_ne!(reread.digest, copy.digest);
    }

    /// The required test: a workflow task's script changing on disk between
    /// launch and resume must refuse resume. Routed through PRODUCTION
    /// discovery (`load_plugin_from_path` → `detect_components` →
    /// `build_plugin_workflow_inventory`, the same `discovered_inventory`
    /// helper every other production-path test in this module uses) so the
    /// launch-time copy is captured from a real discovered
    /// `WorkflowInventoryEntry`, not a bespoke fixture — and the "resume"
    /// side re-reads the SAME file fresh off disk, standing in for
    /// `tools/workflow`'s `std::fs::read(&resolved_path)` at the actual
    /// (out-of-crate) resume call site.
    ///
    /// Two drift cases, deliberately:
    ///  * a SAME-LENGTH edit — the review's plant showed a length-derived
    ///    "digest" passed this test when the only staged drift changed the
    ///    script's length, so the realistic edit is staged FIRST and on its
    ///    own;
    ///  * a length-changing edit, the original case.
    ///
    /// Positive control included: resuming against genuinely UNCHANGED bytes
    /// (re-read fresh, not the stored copy) must succeed — proving the gate
    /// can pass at all, so the refusals below are not merely a gate that
    /// always fails.
    #[tokio::test]
    async fn a_changed_script_digest_refuses_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let root = plugin_root(tmp.path(), r#"{"name":"demo-plugin"}"#);
        let dir = root.join("workflows");
        fs::create_dir_all(&dir).unwrap();
        let script_path = dir.join("resumable.js");
        fs::write(&script_path, script("resumable")).unwrap();

        let inventory = discovered_inventory(&root).await;
        assert_eq!(file_names(&inventory), vec!["resumable.js"]);
        let entry = &inventory[0];

        // LAUNCH: take the copy+digest by reading the discovered script's
        // bytes off disk.
        let copy = TaskScriptCopy::read_at_launch(&entry.path)
            .await
            .expect("discovered script must be readable");
        assert_eq!(copy.script, script("resumable"));

        // Positive control — nothing has changed yet: a fresh re-read still
        // verifies clean, and resume may proceed.
        let unchanged_reread = fs::read_to_string(&entry.path).unwrap();
        assert!(
            verify_resume_digest(&copy, &unchanged_reread).is_ok(),
            "resume must be allowed to proceed when the script has not changed"
        );

        // PAUSE, then the plugin's script file is edited on disk before
        // resume — the exact scenario this task exists to close.
        //
        // Case 1: a SAME-LENGTH edit. `'resumable'` → `'resumabIe'` is one
        // byte different and identical in length — the shape a real "plugin
        // updated between pause and resume" edit usually takes.
        let same_len = script("resumabIe");
        assert_eq!(
            same_len.len(),
            copy.script.len(),
            "fixture setup error: case 1 must be the SAME length as the launch script"
        );
        assert_ne!(same_len, copy.script);
        fs::write(&entry.path, &same_len).unwrap();
        let current = fs::read_to_string(&entry.path).unwrap();
        let err = verify_resume_digest(&copy, &current).expect_err(
            "a same-length script edit must still refuse resume — the digest must be a \
             content digest, not a size fingerprint",
        );
        assert_eq!(err.expected, copy.digest);
        assert_eq!(err.actual, script_digest(&current));
        assert_ne!(err.actual, copy.digest);

        // The refusal's rendered message is what an out-of-crate resume call
        // site would surface; pin that it names BOTH digests and says what it
        // refused, so the eventual user-facing failure identifies the drift.
        let rendered = err.to_string();
        assert!(
            rendered.contains(&copy.digest)
                && rendered.contains(&err.actual)
                && rendered.contains("refusing resume"),
            "mismatch message must name both digests and the refusal: {rendered}"
        );

        // Case 2: a length-changing edit.
        fs::write(&entry.path, script("resumable-but-edited")).unwrap();
        let current = fs::read_to_string(&entry.path).unwrap();
        assert_ne!(
            current.len(),
            copy.script.len(),
            "fixture setup error: case 2 must change the length"
        );
        let err = verify_resume_digest(&copy, &current)
            .expect_err("a changed script's digest must refuse resume");
        assert_eq!(err.expected, copy.digest);
        assert_ne!(
            err.actual, copy.digest,
            "the reported actual digest must be the CHANGED script's digest"
        );
        assert_eq!(err.actual, script_digest(&current));
    }
}
