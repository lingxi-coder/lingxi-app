//! Mobile composition root (M8-P11).
//!
//! The mobile sibling of `engine-desktop`: the single declarative place that
//! decides the mobile capability set. It links a *different* subset of crates
//! via `Cargo.toml` — the cross-platform tools (file/task/web/plan/meta/cron/
//! ui/skill) plus the mobile-exclusive tools (camera/voice/share), the mobile
//! skill set, and the core + mobile command sets — while deliberately omitting
//! the desktop-only tools (shell/agent/mcp/lsp/team/worktree) and the
//! device-control tools. Same core agent logic, different assembly: no
//! `#[cfg(target_os)]` switching in any library crate.
//!
//! As with `engine-desktop`, the host binary owns the runtime wiring
//! (constructing the `BuiltinToolContext` from an `Arc<dyn Platform>` and
//! threading the orchestrator handle); this crate provides the pure
//! registry-assembly functions.

#![forbid(unsafe_code)]

use command_api::{
    BundledPromptFn, CommandFrontmatter, CommandRegistry, CommandSource, SlashCommand,
    SlashCommandKind,
};
use command_core::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2,
    register_core_batch_4, register_core_batch_5,
};
use skill_api::SkillRegistry;
use std::sync::Arc;
use tool_api::{BuiltinToolContext, ToolRegistry};
use tool_ui::ask_user_question::AskUserQuestionResolver;
use traits::{AuthHandle, OrchestratorHandle};

// F3-03: the shared mobile session-host module — `MobileConfig` +
// `build_mobile(MobileConfig, Platform, listener, sink) -> MobileRuntime`. It
// lives under the `uniffi` feature because it wires the `client-adapter` sinks
// (`AdapterOutputStream` / `AdapterPermissionGate`) + the `ClientEventListener`,
// which are pulled ONLY under that feature (the FFI surface). Both FFI packager
// crates (`ios-framework` / `android-aar`) re-export this shared host (F3-04) so
// iOS and Android cannot drift.
#[cfg(feature = "uniffi")]
mod host;

// Audit fix (#14): the disk-backed Skill loader the FFI host wires so the mobile
// Skill tool resolves on-disk `.lingxi/commands` / `.lingxi/skills` under the
// app-private root. uniffi-gated — its `SkillLoader` impl uses `async-trait`
// (an FFI-only optional dep) and only the FFI host constructs a real loader.
#[cfg(feature = "uniffi")]
mod skill_loader;
// v3 Phase 1: workflow-on-mobile composition pieces (launcher + deferred
// invoker), consumed by the `host` build path.
#[cfg(feature = "uniffi")]
mod workflow_support;

// First-party local-app host operations as ORDINARY builtin tools (they used
// to be reachable only as `mcp__local_apps__*`, which gave them third-party
// MCP permission semantics they were never meant to have).
#[cfg(feature = "uniffi")]
pub mod local_apps_tools;

// LOCAL-APPS (phase 1): the domain ⇄ protocol bridge for the engine-owned
// `local_apps::AppService` — the observer that lowers domain events onto the
// client event sink plus the DTO lowering/raising helpers the `submit` command
// arms use. uniffi-gated like `host` (it names the client-protocol DTO surface,
// which is pulled only under that feature).
#[cfg(feature = "uniffi")]
mod local_apps_bridge;

// P1.4: mobile atomic materialization with digest verification (§19.2) for
// the compiled-in builtin plugin bundle the packer (`local_apps::packer`)
// produces. uniffi-gated like its `local_apps_*` siblings — it names
// `local_apps::PackedFile`, which is pulled only under this feature.
#[cfg(feature = "uniffi")]
pub mod builtin_bundle;
#[cfg(feature = "uniffi")]
mod local_app_plugin_binding;
#[cfg(feature = "uniffi")]
mod local_app_runtime_profiles;
#[cfg(feature = "uniffi")]
mod local_apps_build;
#[cfg(feature = "uniffi")]
mod local_apps_host;
// Live per-connection device handles (camera / voice / location /
// notifications) behind a SharedLlm-style swap cell — see the module doc for
// why a bare OnceLock would pin a torn-down engine's Swift objects.
#[cfg(feature = "uniffi")]
mod local_apps_device;
// LOCAL-APPS (v3): the app-facing LLM seam — `LocalAppsModel` + the
// `ApiService`-backed `chat` used by the `llm.chat` bridge operation. The
// designer/generation pipeline that used to live behind this seam is gone.
#[cfg(feature = "uniffi")]
mod local_apps_llm;
#[cfg(feature = "uniffi")]
mod local_apps_mcp;
#[cfg(feature = "uniffi")]
mod local_apps_profile;

#[cfg(feature = "uniffi")]
pub use client_protocol::listings::{
    ModelBillingModeDto, ModelCapabilitiesDto, ModelDetailsDto, ModelPricingDto,
    ModelPricingTierDto,
};
#[cfg(feature = "uniffi")]
pub use host::{
    build_mobile, build_mobile_engine, build_mobile_engine_inner, build_mobile_inner,
    parse_mobile_provider_config_json, CronDueOccurrenceDto, CronFireStatusDto, CronTaskDto,
    FiredCronJobDto, LocalAppBackgroundRunDto, MobileBuildError, MobileConfig,
    MobileCronStoreHandle, MobileEngineError, MobileEngineHandle, MobileOAuthSessionDto,
    MobileOAuthStateDto, MobileRuntime, ProviderCatalogEntryDto, ProviderConnectionTestDto,
};

// F3-06: the host-only walking-skeleton support — a portable fake `Platform`
// shim (fs/http/clock stubs over a temp root), a recording `ClientEventListener`,
// a collecting `PermissionRequestSink`, and a streaming-injecting engine
// constructor. Lives behind the `uniffi` feature (it names the FFI-surface
// types) and is exposed so both the in-crate F3-03/F3-05 unit tests AND the
// `tests/skeleton_test.rs` integration test build the SAME off-device host. The
// real device `Platform` is `cfg(target_os)`-gated, so this shim is what proves
// the skeleton on CI — exactly the spec §8 "prove from a Swift/Kotlin unit test"
// smoke path, runnable on the host.
#[cfg(feature = "uniffi")]
pub mod test_support;

// F3-04: re-export the FFI-visible adapter types both packager crates name when
// they call `build_mobile_engine` (the foreign `ClientEventListener` they
// register and the `PermissionRequestSink` the gate emits to). Re-exporting them
// from the shared host crate keeps the FFI crates free of a direct
// `client-adapter` import for these types — the shared host is the single seam.
#[cfg(feature = "uniffi")]
pub use client_adapter::{ClientEventListener, ListenerSink, PermissionRequestSink};

// F3-04: this crate now DEFINES UniFFI-exported types (`MobileEngineHandle` as a
// `uniffi::Object`, `MobileEngineError` as a `uniffi::Error` — see `host`), so it
// must register their FFI metadata via the scaffolding macro. The aggregating
// cdylib crates (`ios-framework` / `android-aar`) re-export this scaffolding so
// the symbols land in the final library (the same pattern `client-adapter` uses
// for the `ClientEventListener` callback interface). Compiles ONLY under the
// `uniffi` feature; the default host build never includes it.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// P1.6 (§19.2) — mobile composes `PluginManager` for the first time.
//
// Mobile's plugin surface is exactly ONE compiled-in plugin
// (`lingxi-local-app`, the on-device Local App authoring/build/test workflow
// bundle under `plugins/lingxi-local-app/`). Unlike `engine-desktop`, mobile
// has no `~/.claude/plugins` cache to scan, no marketplace, and no
// `--plugin-dir`/`--add-dir` session surface — so mobile never calls
// `PluginManager::install` at all. The single compiled-in plugin is
// registered through `register_verified_builtin`, the door P0a.6 opened
// specifically for a plugin whose `(id, manifest, install_dir)` triple never
// passed through untrusted, attacker-influenced input.
//
// `register_mobile_builtin_plugin` refuses anything whose `manifest.source`
// is not already `PluginSource::BuiltIn` — defense in depth from the
// opposite direction of `PluginManager::install`'s own unconditional
// rejection of that SAME variant, so a manifest that ever passed through a
// network/local-path install arm cannot be smuggled through this door and
// come out re-labeled trusted.
//
// What this task deliberately leaves undone, because it is out of scope for
// this task's owned files (`Cargo.toml` / `lib.rs` /
// `local_app_plugin_binding.rs`):
//   - Wiring this into `host::build_mobile_inner` — `host.rs` is owned by a
//     different task/session; the functions below are ready for it to call
//     but nothing calls them yet outside this file's own tests.
//   - Embedding the plugin's real components (skills/agents/workflows under
//     `plugins/lingxi-local-app/`) — `manifest.components` is left at
//     `PluginComponents::default()` until the compiled-in bundle
//     (`builtin_bundle`/`local_apps::packer`) has a materialized on-device
//     root for this manifest to point components at. That is not a load
//     failure today: `PluginManager::load_plugin` materializes zero
//     components and returns `Ok` when a manifest declares none.
//   - §19.0's binary-size/startup budgets
//     (`local-apps/src/performance_thresholds.rs`'s `load_baseline()`, which
//     has no caller anywhere in the workspace) start binding once `plugin`
//     is linked in by this task. The natural call site is inside
//     `host::build_mobile_inner`, right after it wires this plugin edge in —
//     not here, since this file never constructs the real on-device
//     `MobileRuntime`.
/// The one plugin name mobile ever discovers (§19.2's completion condition).
#[cfg(feature = "uniffi")]
pub const MOBILE_BUILTIN_PLUGIN_NAME: &str = "lingxi-local-app";

/// The compiled-in identity of that one plugin, as a fixed UUID.
///
/// Deliberately a CONSTANT, not `PluginId::new()`. `PluginId` is the key that
/// `PluginManager`'s state map, `disable`/`unload_plugin`, and — security
/// relevantly — `PluginBlocklist::is_blocked` all match on. A fresh v4 UUID
/// per call would mean:
///   - two `register_mobile_builtin_plugins` calls against one manager
///     register the SAME plugin twice, under two different ids, so
///     "mobile loads exactly one plugin" stops holding at the manager;
///   - `disable`/`unload_plugin` cannot be handed a stable id by anything
///     that did not itself just call [`mobile_builtin_plugins`]; and
///   - no managed-settings blocklist entry could ever name this plugin,
///     because its id would differ on every boot. `plugin::manager`'s own
///     `register_verified_builtin` doc calls out exactly that failure ("…
///     `PluginId::new()` is a fresh v4 UUID per process, so the check runs
///     but can only match an id the host minted and blocked within the same
///     run").
///
/// The bytes are hand-picked (`6c69 6e67 7869 4c41` is ASCII `lingxiLA`) so
/// the value is visibly a compiled-in constant rather than a captured random
/// id somebody could be tempted to "refresh".
#[cfg(feature = "uniffi")]
pub const MOBILE_BUILTIN_PLUGIN_UUID: u128 = 0x6c69_6e67_7869_4c41_0000_0000_0000_0001;

/// The compiled-in [`protocol::PluginId`] for [`MOBILE_BUILTIN_PLUGIN_NAME`].
#[cfg(feature = "uniffi")]
#[must_use]
pub fn mobile_builtin_plugin_id() -> protocol::PluginId {
    protocol::PluginId::from_uuid(uuid::Uuid::from_u128(MOBILE_BUILTIN_PLUGIN_UUID))
}

/// Build the compiled-in Local App plugin's manifest.
///
/// `source` is stamped `PluginSource::BuiltIn` here directly (and re-stamped
/// by `register_verified_builtin` unconditionally regardless, so this can
/// never silently drift from it) so [`mobile_builtin_plugins`] itself already
/// carries the completion condition's shape — name and source — without
/// waiting on registration against a live `PluginManager`.
#[cfg(feature = "uniffi")]
fn mobile_builtin_plugin_manifest() -> (protocol::PluginId, plugin::PluginManifest) {
    let id = mobile_builtin_plugin_id();
    let manifest = plugin::PluginManifest {
        id,
        name: MOBILE_BUILTIN_PLUGIN_NAME.to_string(),
        display_name: None,
        default_enabled: true,
        version: "1.0.0".to_string(),
        description: "On-device Local App authoring/build/test workflow set.".to_string(),
        author: Some(branding::PRODUCT_NAME.to_string()),
        homepage: None,
        source: plugin::PluginSource::BuiltIn,
        components: plugin::PluginComponents::default(),
        trust_level: plugin::default_trust_for_source(&plugin::PluginSource::BuiltIn),
        depends_on: Vec::new(),
        dependencies: Vec::new(),
        user_config: None,
        channels: Vec::new(),
        settings: std::collections::HashMap::new(),
    };
    (id, manifest)
}

/// Every plugin mobile ships, compiled in. Exactly one today: the pinned
/// [`MOBILE_BUILTIN_PLUGIN_NAME`] builtin (§19.2's completion condition).
/// Mobile deliberately has no disk-scan/marketplace/session-plugin discovery
/// path, so this IS the entire mobile plugin surface, not a subset of some
/// larger discovered set a caller is expected to filter.
#[cfg(feature = "uniffi")]
#[must_use]
pub fn mobile_builtin_plugins() -> Vec<(
    protocol::PluginId,
    plugin::PluginManifest,
    std::path::PathBuf,
)> {
    let (id, manifest) = mobile_builtin_plugin_manifest();
    vec![(id, manifest, std::path::PathBuf::new())]
}

/// Register one mobile-builtin plugin into `manager` through the verified-
/// builtin door, refusing anything whose `manifest.source` is not already
/// `PluginSource::BuiltIn`.
///
/// This refusal is mobile-specific defense in depth, not something
/// `register_verified_builtin` itself enforces (that method stamps `source`
/// to `BuiltIn` unconditionally by design, trusting a caller that already
/// did its own verification before calling it). Gating here too means a
/// manifest that ever passed through a network/local-path install arm cannot
/// reach this door and quietly come out re-labeled trusted.
#[cfg(feature = "uniffi")]
pub async fn register_mobile_builtin_plugin(
    manager: &plugin::PluginManager,
    id: &protocol::PluginId,
    manifest: plugin::PluginManifest,
    install_dir: std::path::PathBuf,
) -> Result<(), plugin::PluginManagerError> {
    if !matches!(manifest.source, plugin::PluginSource::BuiltIn) {
        return Err(plugin::PluginManagerError::Io(format!(
            "refusing to register {:?} through the mobile builtin-plugin door: \
             source must be PluginSource::BuiltIn, got {:?}",
            manifest.name, manifest.source
        )));
    }
    manager
        .register_verified_builtin(id, manifest, install_dir)
        .await
}

/// Register every mobile-builtin plugin (today: exactly the one
/// [`mobile_builtin_plugins`] names) into `manager`.
#[cfg(feature = "uniffi")]
pub async fn register_mobile_builtin_plugins(
    manager: &plugin::PluginManager,
) -> Result<(), plugin::PluginManagerError> {
    for (id, manifest, install_dir) in mobile_builtin_plugins() {
        register_mobile_builtin_plugin(manager, &id, manifest, install_dir).await?;
    }
    Ok(())
}

#[cfg(all(test, feature = "uniffi"))]
mod mobile_plugin_composition_tests {
    use super::*;
    use plugin::{PluginManager, PluginSource};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// A fully-wired `PluginManager` (every registry live, no mocks) rooted at
    /// `root` — the same shape `plugin::manager::register_verified_builtin_tests`'s
    /// own `build_manager` uses, built from `platform-posix-minimal`'s fakes
    /// (already an engine-mobile dependency) instead of the full `platforms/posix`
    /// crate desktop links.
    fn build_manager(root: &std::path::Path) -> PluginManager {
        let credentials = Arc::new(secret::CredentialManager::new(
            Arc::new(platform_posix_minimal::PlainTextSecureStorage::new()),
            Arc::new(platform_posix_minimal::PosixClock::new()),
            Arc::new(platform_posix_minimal::PosixHttp::new()),
        ));
        PluginManager::new(
            root.to_path_buf(),
            Arc::new(platform_posix_minimal::PosixFileSystem::new(
                root.to_path_buf(),
            )),
            Arc::new(platform_posix_minimal::PosixHttp::new()),
            Arc::new(platform_posix_minimal::PosixRuntime::new()),
            credentials,
            Arc::new(plugin::PluginBlocklist::new(String::new())),
            Arc::new(plugin::StrictPluginOnlyPolicy::empty()),
            Arc::new(RwLock::new(CommandRegistry::new())),
            Arc::new(RwLock::new(SkillRegistry::new())),
            Arc::new(RwLock::new(hooks::HookRegistry::new())),
            Arc::new(RwLock::new(outputstyles::OutputStyleRegistry::new())),
            Arc::new(mcp::McpRegistry::new(Arc::new(
                platform_posix_minimal::PosixMcp::new(),
            ))),
            Arc::new(lsp::LspRegistry::new(Arc::new(
                platform_posix_minimal::PosixLsp::new(),
            ))),
            Arc::new(RwLock::new(ToolRegistry::new())),
        )
    }

    /// §19.2's completion condition, asserted against PRODUCTION discovery —
    /// [`mobile_builtin_plugins`] — not a hand-built vector standing in for
    /// it, and against the plugin's OWN name, not merely a count.
    #[tokio::test]
    async fn mobile_discovers_exactly_one_plugin() {
        let discovered = mobile_builtin_plugins();
        assert_eq!(
            discovered.len(),
            1,
            "mobile must discover exactly one compiled-in plugin"
        );
        let (discovered_id, manifest, _) = &discovered[0];
        // Against the LITERAL §19.2 pins, not against
        // `MOBILE_BUILTIN_PLUGIN_NAME` — the manifest is BUILT from that
        // constant, so `manifest.name == MOBILE_BUILTIN_PLUGIN_NAME` is a
        // tautology that stays green through any rename. The completion
        // condition names a specific string; the test has to too.
        assert_eq!(
            manifest.name, "lingxi-local-app",
            "the one discovered plugin must be named lingxi-local-app, not merely \
             counted"
        );
        assert!(
            matches!(manifest.source, PluginSource::BuiltIn),
            "the one discovered plugin must be sourced BuiltIn, got {:?}",
            manifest.source
        );

        // Round-trip through a REAL `PluginManager`: if `mobile_builtin_plugins`
        // ever regressed to returning an empty list (the House Defect this
        // brief warns about — "a discovery path that finds nothing"),
        // `register_mobile_builtin_plugins` would register nothing and
        // `loaded_plugin_ids` would come back empty, failing this too — so
        // this is not satisfied by the manifest list alone.
        let tmp = tempfile::tempdir().expect("temp install dir");
        let manager = build_manager(tmp.path());
        register_mobile_builtin_plugins(&manager)
            .await
            .expect("the mobile builtin plugin must register cleanly");
        // NAME what was counted: the Loaded set must be exactly the id
        // production discovery returned, not "some one plugin". A bare
        // `len() == 1` cannot tell the builtin apart from anything else that
        // happened to end up Loaded.
        assert_eq!(
            manager.loaded_plugin_ids().await,
            vec![*discovered_id],
            "the Loaded set after mobile's plugin boot must be exactly the id \
             mobile_builtin_plugins() returned"
        );
    }

    /// The compiled-in plugin's `PluginId` must be a STABLE constant, not a
    /// fresh v4 per call.
    ///
    /// `PluginId` is what `PluginManager`'s state map, `disable`/
    /// `unload_plugin` and `PluginBlocklist::is_blocked` all key on. This is
    /// the paired non-vacuous case for `mobile_discovers_exactly_one_plugin`,
    /// whose `len() == 1` is trivially satisfied because it registers exactly
    /// once — an id that changes per call keeps that assertion green while
    /// breaking the invariant it is supposed to stand for.
    #[tokio::test]
    async fn mobile_builtin_plugin_identity_is_stable_across_calls() {
        let first = mobile_builtin_plugins();
        let second = mobile_builtin_plugins();
        assert_eq!(
            first[0].0, second[0].0,
            "two calls to mobile_builtin_plugins() must yield the SAME PluginId; \
             a per-call id makes the blocklist unfireable and double-registers \
             the plugin"
        );
        assert_eq!(
            first[0].0,
            mobile_builtin_plugin_id(),
            "discovery must hand out the compiled-in constant id"
        );
        // And the constant itself is FROZEN, pinned against a literal rather
        // than against `MOBILE_BUILTIN_PLUGIN_UUID` (which would be a
        // tautology): this id is what per-plugin settings, disable state and
        // blocklist entries are keyed by on device, so changing it silently
        // orphans all of them across an app upgrade.
        assert_eq!(
            mobile_builtin_plugin_id().to_string(),
            "plg:6c696e67-7869-4c41-0000-000000000001",
            "the compiled-in plugin id is an on-device identity; changing it \
             orphans every persisted per-plugin record keyed by it"
        );

        // The observable consequence: registering mobile's plugin set twice
        // against ONE manager must leave exactly one Loaded plugin. With a
        // per-call id the second pass inserts a second state-map entry under
        // a second id and this comes back with two.
        let tmp = tempfile::tempdir().expect("temp install dir");
        let manager = build_manager(tmp.path());
        register_mobile_builtin_plugins(&manager)
            .await
            .expect("first registration must succeed");
        register_mobile_builtin_plugins(&manager)
            .await
            .expect("re-registering the compiled-in plugin must be idempotent");
        assert_eq!(
            manager.loaded_plugin_ids().await,
            vec![mobile_builtin_plugin_id()],
            "registering mobile's builtin set twice must leave exactly the one \
             compiled-in id Loaded, not two entries for the same plugin"
        );
    }

    /// The negative half: a manifest whose source is anything OTHER than
    /// `PluginSource::BuiltIn` must be refused by mobile's own registration
    /// door, not merely "the one plugin present happens to be BuiltIn"
    /// (which a test that never tries a second source cannot distinguish from
    /// "there was nothing else to reject").
    #[tokio::test]
    async fn mobile_loads_only_the_builtin_source() {
        let tmp = tempfile::tempdir().expect("temp install dir");
        let manager = build_manager(tmp.path());

        // Positive control FIRST, so the "was anything rejected?" question is
        // answered against a NON-EMPTY manager. Asserting "zero loaded" over a
        // manager that never had anything in it cannot distinguish "the
        // refusal registered nothing" from "there was nothing here anyway".
        register_mobile_builtin_plugins(&manager)
            .await
            .expect("the real mobile builtin (source already BuiltIn) must register cleanly");
        let builtin_id = mobile_builtin_plugin_id();
        assert_eq!(
            manager.loaded_plugin_ids().await,
            vec![builtin_id],
            "the door must ACCEPT a BuiltIn source — otherwise the refusals below \
             would just be 'this door rejects everything'"
        );

        // Now the negatives: the SAME manifest shape mobile ships, under a
        // DISTINCT id (so a smuggled-in plugin would ADD a state-map entry
        // rather than silently overwrite the builtin's), with `source` swapped
        // to each non-`BuiltIn` variant in turn — as if it had reached this
        // call site via a network / local-path install arm instead of mobile's
        // own verified-builtin construction. Two variants, not one, so a guard
        // that ever narrowed to a single rejected variant is caught.
        let rogue_sources = [
            PluginSource::LocalPath {
                path: std::path::PathBuf::from("/definitely-not-verified"),
            },
            PluginSource::OfficialMarketplace {
                name: MOBILE_BUILTIN_PLUGIN_NAME.to_string(),
            },
        ];
        let rogue_id =
            protocol::PluginId::from_uuid(uuid::Uuid::from_u128(MOBILE_BUILTIN_PLUGIN_UUID + 1));
        assert_ne!(
            rogue_id, builtin_id,
            "the rogue must carry an id of its own, or a successful smuggle \
             would overwrite the builtin instead of showing up as an extra entry"
        );
        let mut refusals = 0_usize;
        for rogue_source in rogue_sources {
            refusals += 1;
            let (_, mut manifest) = mobile_builtin_plugin_manifest();
            manifest.source = rogue_source.clone();

            let error = register_mobile_builtin_plugin(
                &manager,
                &rogue_id,
                manifest,
                tmp.path().to_path_buf(),
            )
            .await
            .expect_err("a non-BuiltIn source must be refused, not silently loaded");
            match error {
                plugin::PluginManagerError::Io(msg) => assert!(
                    msg.contains("BuiltIn") && msg.contains(MOBILE_BUILTIN_PLUGIN_NAME),
                    "refusal must name both the required source and the refused plugin: {msg}"
                ),
                other => panic!("expected PluginManagerError::Io, got {other:?}"),
            }

            // The refusal must have changed nothing: the builtin is still the
            // ONE Loaded plugin and the rogue id is absent. `register_verified_builtin`
            // stamps `source = BuiltIn` unconditionally, so without the guard
            // this rogue would land here re-labeled trusted and this comes back
            // with two ids.
            assert_eq!(
                manager.loaded_plugin_ids().await,
                vec![builtin_id],
                "a refused {rogue_source:?} must leave the Loaded set exactly as it \
                 was — only the compiled-in builtin"
            );
        }

        // Probe-fired control: everything above lives inside a `for` over a
        // fixture list, so an empty (or accidentally-emptied) `rogue_sources`
        // would make this whole test pass having rejected nothing at all.
        assert_eq!(
            refusals, 2,
            "the refusal body must actually have run, once per non-BuiltIn \
             variant — a test that rejects nothing proves nothing"
        );
    }
}

/// Mobile engine knobs.
#[derive(Clone, Debug)]
pub struct MobileEngineConfig {
    /// Model id the mobile build defaults to.
    pub default_model: String,
}

impl Default for MobileEngineConfig {
    fn default() -> Self {
        Self {
            // The host's boot default for the Anthropic route. Keep it
            // provider-qualified so the shared Claude model ids exposed by
            // Copilot cannot make the fresh-session default ambiguous.
            default_model: traits::qualified_model_ref(
                traits::provider_default_model("anthropic").unwrap_or("claude-sonnet-5"),
                Some("anthropic"),
            ),
        }
    }
}

/// Assemble the mobile builtin **tool** registry from a freshly-built
/// [`BuiltinToolContext`] (whose `camera`/`voice`/`share` handles come from the
/// mobile `Platform`).
#[must_use]
pub fn mobile_tool_registry(ctx: BuiltinToolContext) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_tools(&mut reg, ctx);
    reg
}

/// Register the mobile tool set into an existing registry, with the `Skill` tool
/// INERT (the hermetic `EmptySkillLoader`). Used by tests and the non-FFI host
/// build. The FFI host instead calls [`register_mobile_tools_with_skill_loader`]
/// to wire a disk-backed loader (audit fix #14).
pub fn register_mobile_tools(reg: &mut ToolRegistry, ctx: BuiltinToolContext) {
    register_mobile_non_skill_tools_with_ask_resolver(reg, ctx.clone(), None, None);
    // Skill tool with the hermetic `EmptySkillLoader` (no on-disk discovery).
    tool_skill::register_all(reg, ctx);
}

/// Audit fix (#14): register the mobile tool set with a FUNCTIONAL `Skill` tool
/// backed by `skill_loader` (the disk-backed `MobileDiskSkillLoader`) instead of
/// the inert `EmptySkillLoader`, so model-invoked skills resolve against the
/// device's on-disk `.lingxi/commands` / `.lingxi/skills`. uniffi-gated because
/// the loader impl needs `async-trait` (an FFI-only optional dep) and only the
/// FFI host wires a real loader.
#[cfg(feature = "uniffi")]
pub fn register_mobile_tools_with_skill_loader(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    config_home: std::path::PathBuf,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
) {
    register_mobile_non_skill_tools_with_ask_resolver(reg, ctx.clone(), Some(config_home), None);
    reg.register_builtin(Arc::new(tool_skill::SkillTool::with_loader(
        ctx,
        skill_loader,
    )));
}

/// Every mobile tool EXCEPT `Skill` (whose loader differs by build). Builtin
/// wire order is locale-sorted at enumeration time, so registration order is
/// immaterial. Hosts that surface a live questionnaire bridge inject a custom
/// `AskUserQuestion` resolver here so the registry never contains duplicate
/// same-name builtins.
fn register_mobile_non_skill_tools_with_ask_resolver(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    config_home: Option<std::path::PathBuf>,
    ask_resolver: Option<Arc<dyn AskUserQuestionResolver>>,
) {
    // ----- cross-platform subset (also linked by engine-desktop) -----------
    tool_file::register_all(reg, ctx.clone());
    if let Some(config_home) = config_home {
        tool_task::register_mobile_with_config_home(reg, ctx.clone(), config_home);
    } else {
        tool_task::register_mobile(reg, ctx.clone());
    }
    tool_web::register_all(reg, ctx.clone(), None);
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    // Audit fix (#7): the cron tools (Create/List/Delete/RemoteTrigger) are
    // registered, but mobile starts NO `cron::CronScheduler` (the desktop root is
    // the only place one runs) and wires no `task_registry` for it to fire into —
    // a backgrounded app has no long-running daemon. So a created cron job is
    // saved/listed/deletable but does NOT auto-fire on this platform; CronCreate's
    // result text says so (see schedule_cron.rs `scheduler_active`). RemoteTrigger
    // is independent of the local scheduler (it triggers a cloud-side run).
    tool_cron::register_all(reg, ctx.clone());
    match ask_resolver {
        Some(resolver) => tool_ui::register_all_with_ask_resolver(reg, ctx.clone(), resolver),
        None => tool_ui::register_all(reg, ctx.clone()),
    }
    // ----- mobile-exclusive tools ------------------------------------------
    // camera / voice / speech / notification / clipboard / share, folded into
    // the single `tool-mobile` crate.
    tool_mobile::register_all(reg, ctx.clone());
    // P3/P4: mobile shell tool. The composition root pre-gates it so a selected
    // but unavailable mobile-linux runtime never silently falls back to legacy.
    tool_shell_mobile::register_all(reg, ctx.clone());
    // P4: mobile structured git tool. Same pre-gate rule as shell.
    tool_git_mobile::register_all(reg, ctx);
}

/// Audit fix (#14): the FFI sibling of [`mobile_tool_registry`] that wires a
/// disk-backed `Skill` loader (the mobile composition root passes the
/// `MobileDiskSkillLoader` it built from the device's app-private root).
#[cfg(feature = "uniffi")]
#[must_use]
pub fn mobile_tool_registry_with_skill_loader(
    ctx: BuiltinToolContext,
    config_home: std::path::PathBuf,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_tools_with_skill_loader(&mut reg, ctx, config_home, skill_loader);
    reg
}

#[cfg(feature = "uniffi")]
#[must_use]
/// FFI host variant of [`mobile_tool_registry_with_skill_loader`] that installs
/// a live `AskUserQuestion` resolver exactly once, preserving builtin lookup
/// order while keeping the disk-backed `Skill` loader.
pub fn mobile_tool_registry_with_skill_loader_and_ask_resolver(
    ctx: BuiltinToolContext,
    config_home: std::path::PathBuf,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
    ask_resolver: Arc<dyn AskUserQuestionResolver>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_non_skill_tools_with_ask_resolver(
        &mut reg,
        ctx.clone(),
        Some(config_home),
        Some(ask_resolver),
    );
    reg.register_builtin(Arc::new(tool_skill::SkillTool::with_loader(
        ctx,
        skill_loader,
    )));
    reg
}

/// Register Android Computer Use only when both the Direct-build Cargo feature
/// and a live native host are present. Play builds pass no host and compile
/// without the feature. Direct foreground and headless engines intentionally
/// share the host; its in-memory active-session grants remain the security gate.
pub fn register_android_ui_automation(
    reg: &mut ToolRegistry,
    automation: Option<Arc<dyn traits::AndroidUiAutomation>>,
) {
    #[cfg(feature = "android-computer-use")]
    if let Some(automation) = automation {
        tool_android_use::register_all(reg, automation);
    }
    #[cfg(not(feature = "android-computer-use"))]
    let _ = (reg, automation);
}

#[cfg(test)]
mod android_ui_registration_tests {
    use super::register_android_ui_automation;
    use async_trait::async_trait;
    use std::sync::Arc;
    use traits::{
        AndroidAccessRequest, AndroidAction, AndroidActionResult, AndroidAppInfo,
        AndroidAutomationError, AndroidAutomationStatus, AndroidNodeQuery, AndroidScreenshot,
        AndroidUiAutomation, AndroidUiNode, AndroidUiSnapshot, AndroidWaitCondition,
    };

    struct StubAutomation;

    fn unsupported<T>() -> Result<T, AndroidAutomationError> {
        Err(AndroidAutomationError::Unsupported("test stub".into()))
    }

    #[async_trait]
    impl AndroidUiAutomation for StubAutomation {
        async fn status(&self) -> Result<AndroidAutomationStatus, AndroidAutomationError> {
            unsupported()
        }

        async fn request_access(
            &self,
            _request: AndroidAccessRequest,
        ) -> Result<Vec<AndroidAppInfo>, AndroidAutomationError> {
            unsupported()
        }

        async fn list_granted_apps(&self) -> Result<Vec<AndroidAppInfo>, AndroidAutomationError> {
            unsupported()
        }

        async fn screenshot(&self) -> Result<AndroidScreenshot, AndroidAutomationError> {
            unsupported()
        }

        async fn ui_tree(&self) -> Result<AndroidUiSnapshot, AndroidAutomationError> {
            unsupported()
        }

        async fn find_nodes(
            &self,
            _query: AndroidNodeQuery,
        ) -> Result<Vec<AndroidUiNode>, AndroidAutomationError> {
            unsupported()
        }

        async fn inspect_node(
            &self,
            _node_id: String,
        ) -> Result<AndroidUiNode, AndroidAutomationError> {
            unsupported()
        }

        async fn perform(
            &self,
            _action: AndroidAction,
        ) -> Result<AndroidActionResult, AndroidAutomationError> {
            unsupported()
        }

        async fn wait_for(
            &self,
            _condition: AndroidWaitCondition,
            _timeout_ms: u64,
        ) -> Result<AndroidActionResult, AndroidAutomationError> {
            unsupported()
        }

        async fn stop(&self) -> Result<(), AndroidAutomationError> {
            Ok(())
        }
    }

    #[test]
    fn registration_follows_the_direct_feature_gate() {
        let mut registry = tool_api::ToolRegistry::new();
        register_android_ui_automation(&mut registry, Some(Arc::new(StubAutomation)));
        #[cfg(feature = "android-computer-use")]
        assert!(registry.find_by_name("android_use").is_some());
        #[cfg(not(feature = "android-computer-use"))]
        assert!(registry.find_by_name("android_use").is_none());
    }
}

/// Assemble the mobile builtin **skill** registry.
#[must_use]
pub fn mobile_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_api::register_mobile(&mut reg);
    reg
}

/// Assemble the mobile slash-command registry: the core handlers plus the
/// mobile-only handlers (`/mobile` `/voice` `/share` `/camera`).
#[must_use]
pub fn mobile_command_registry(
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_mobile_bundled_prompt_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    // Batch 8 (`/fork`, `/goal`, `/recap`, `/reload-skills`, `/skill-doctor`,
    // `/stop`) is wired by the uniffi composition root (`host::build_mobile`)
    // right after this returns, because it needs the shared
    // `Arc<tokio::sync::RwLock<CommandRegistry>>` slot (tokio is a
    // `uniffi`-gated optional dependency here, unavailable in this default-lean
    // lib build).
    //
    // Mobile-only command handlers: currently none — the mobile command names
    // (/mobile, /voice, /share, /camera) are served as command-core
    // unimplemented stubs. Register real mobile handlers on `reg` directly here
    // when implemented.
    reg
}

/// Register the mobile authoritative bundled prompt catalog: the command-core
/// programmatic bundled prompts (for example `/loop`) plus the mobile-only
/// bundled skill set. Call this after any disk-sourced load on mobile so the
/// shipped bundled prompts keep precedence over same-name on-device decoys.
pub(crate) fn register_mobile_bundled_prompt_commands(reg: &mut CommandRegistry) {
    // Bundled programmatic skills (`/loop`), mirroring desktop. Gated on the cron
    // kill-switch (loop.ts:83); mobile starts no cron scheduler so a scheduled
    // job is inert, but the skill's listing/usage path is harmless and faithful.
    let cron_enabled =
        !traits::env::is_env_truthy(std::env::var("LINGXI_DISABLE_CRON").ok().as_deref());
    command_core::register_bundled_skills(reg, cron_enabled);
    register_mobile_skill_commands(reg);
}

/// Mirror the compiled-in mobile Skill registry into the slash-command
/// catalog. The Skill tool remains the canonical invocation path, but a
/// settings screen and `/` palette must see the same shipped mobile skills —
/// so this iterates `skills` itself rather than carrying a second, independent
/// name list that could drift out of sync with it (and, before this, could
/// silently skip a name the list still mentioned but the registry had dropped,
/// or simply never mention a name the registry had gained). The prompt body is
/// the exact bundled content, and both invocation paths use the standard
/// argument expansion semantics so they receive identical guidance.
pub(crate) fn register_mobile_skill_commands(reg: &mut CommandRegistry) {
    register_mobile_skill_commands_from(reg, &mobile_skill_registry());
}

/// The derivation itself, parameterised on the registry it mirrors so tests
/// can grow or shrink it independently of the compiled-in mobile skill set —
/// see `mobile_skill_list_is_derived_from_the_live_registry` below.
/// `skills.get(name)` can never miss here: `name` always comes from
/// `skills.names()` on the very same registry, so a skill present in `skills`
/// but absent from the mirrored command set is now a structural
/// impossibility rather than a silent `continue`.
fn register_mobile_skill_commands_from(reg: &mut CommandRegistry, skills: &SkillRegistry) {
    for name in skills.names() {
        let skill = skills
            .get(name)
            .expect("name was just read from this registry's own names()");
        reg.register_command(SlashCommand {
            name: skill.name.clone(),
            description: skill.description.clone(),
            source: CommandSource::Bundled,
            kind: SlashCommandKind::Bundled {
                frontmatter: CommandFrontmatter::default(),
                prompt_fn: Some(Arc::new(MobileSkillPrompt {
                    body: skill.content.clone(),
                })),
            },
            loaded_from: Some("bundled".into()),
            user_invocable: Some(true),
            has_user_specified_description: true,
            ..SlashCommand::default()
        });
    }
}

struct MobileSkillPrompt {
    body: String,
}

impl BundledPromptFn for MobileSkillPrompt {
    fn build(&self, args: &str) -> String {
        command_api::substitute_arguments_faithful(&self.body, Some(args), true, &[])
            .expect("bundled mobile skills have no named arguments")
    }
}

#[cfg(test)]
mod mobile_skill_command_tests {
    use super::*;

    #[test]
    fn slash_skills_are_all_registered_and_keep_complete_bundled_content() {
        let mut commands = CommandRegistry::new();
        register_mobile_skill_commands(&mut commands);

        let mut names: Vec<_> = commands
            .list_all()
            .into_iter()
            .map(|command| command.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "accessibility",
                "babylon-3d-local-app",
                "canvas-2d-local-app",
                "create-local-app",
                "frontend-design",
                "frontend-qa",
                "ionic-react-local-app",
                "phaser-2d-local-app",
                "react-best-practices",
                "threejs-local-app",
            ]
        );

        let skills = mobile_skill_registry();
        // D2 value-level companion to
        // `mobile_skill_command_wrapper_stays_an_unfiltered_delegation` below:
        // the mirrored command set must equal the LIVE registry's name set,
        // computed here rather than typed out. The hardcoded vector above pins
        // WHICH ten skills ship today; this pins that the mirror is TOTAL over
        // whatever the registry actually holds, so the day an eleventh bundled
        // skill lands, a filter anywhere on the production path that silently
        // drops it fails here by value.
        let mut live_names = skills.names();
        live_names.sort_unstable();
        assert_eq!(
            names, live_names,
            "the slash mirror must cover exactly the live mobile skill registry, \
             no more and no less"
        );
        let test_cases = [
            ("ionic-react-local-app", ""),
            ("canvas-2d-local-app", "focus"),
            ("threejs-local-app", "  focus  "),
            ("phaser-2d-local-app", "focus"),
            ("babylon-3d-local-app", "focus"),
            ("ionic-react-local-app", " \t "),
        ];
        for (name, args) in test_cases {
            let command = commands.resolve(name).expect("bundled slash skill");
            let SlashCommandKind::Bundled {
                prompt_fn: Some(prompt),
                ..
            } = &command.kind
            else {
                panic!("{name} must be a bundled prompt command");
            };
            let expected = command_api::substitute_arguments_faithful(
                &skills.get(name).expect("bundled skill").content,
                Some(args),
                true,
                &[],
            )
            .expect("bundled mobile skills have no named arguments");
            let built = prompt.build(args);
            if args.is_empty() {
                assert_eq!(
                    expected,
                    skills.get(name).expect("bundled skill").content,
                    "empty args must leave the bundled body unchanged"
                );
                assert!(
                    !built.contains("\n\nARGUMENTS:"),
                    "empty args must not append an ARGUMENTS footer"
                );
            }
            assert_eq!(
                built, expected,
                "slash invocation must match Skill's standard argument expansion"
            );
            if !args.is_empty() {
                assert!(
                    built.ends_with(&format!("\n\nARGUMENTS: {args}")),
                    "nonempty args must reach the built prompt with the exact raw ARGUMENTS footer"
                );
            }
            if args == " \t " {
                assert_ne!(
                    built,
                    skills.get(name).expect("bundled skill").content,
                    "whitespace-only args currently count as nonempty and must keep the raw ARGUMENTS footer"
                );
            }
        }
    }

    /// P-1.5 — `register_mobile_skill_commands` must derive its slash-command
    /// set from `mobile_skill_registry()` itself, never from a second,
    /// independent name list.
    ///
    /// A value-only comparison against the current ten names cannot catch a
    /// reintroduced hardcoded list: a regression would almost certainly carry
    /// exactly those ten names back (they are exactly what this task removes),
    /// so the output would look identical either way. The only place the two
    /// shapes actually diverge is a skill the list was never told about: a
    /// hardcoded array skips it via the `continue` this task removed; deriving
    /// from `skills.names()` cannot. So this test GROWS the registry with an
    /// eleventh skill the original ten-name array never mentioned, and
    /// requires it to gain a slash command anyway.
    #[test]
    fn mobile_skill_list_is_derived_from_the_live_registry() {
        let planted_name = "planted-eleventh-mobile-skill";
        let mut grown = mobile_skill_registry();
        assert!(
            grown.get(planted_name).is_none(),
            "fixture name must not already collide with a real bundled skill"
        );
        grown.register(skill_api::Skill {
            name: planted_name.to_string(),
            description: "planted for mobile_skill_list_is_derived_from_the_live_registry"
                .to_string(),
            frontmatter: skill_api::SkillFrontmatter {
                name: planted_name.to_string(),
                description: "planted".to_string(),
                ..Default::default()
            },
            content: "planted content".to_string(),
            source: skill_api::SkillSource::Bundled,
            loaded_from: skill_api::LoadedFrom::Bundled,
            plugin_id: None,
            file_path: "<planted-for-test>".into(),
        });

        let mut commands = CommandRegistry::new();
        register_mobile_skill_commands_from(&mut commands, &grown);

        assert!(
            commands.resolve(planted_name).is_some(),
            "a skill registered into the live registry after the ten-name array \
             was removed must automatically gain a slash command — if this \
             fails, something is once again filtering the mirror through a \
             name list independent of `skills`, which would silently skip any \
             skill that list does not mention"
        );

        // The original ten must still be present alongside the planted one —
        // growing the registry must ADD to the mirrored set, not replace it.
        let names: std::collections::BTreeSet<&str> = commands
            .list_all()
            .into_iter()
            .map(|command| command.name.as_str())
            .collect();
        for original in [
            "create-local-app",
            "frontend-design",
            "frontend-qa",
            "accessibility",
            "react-best-practices",
            "ionic-react-local-app",
            "canvas-2d-local-app",
            "threejs-local-app",
            "phaser-2d-local-app",
            "babylon-3d-local-app",
        ] {
            assert!(
                names.contains(original),
                "{original} must still be mirrored alongside the planted skill"
            );
        }
        assert_eq!(
            names.len(),
            11,
            "expected the original ten plus the planted skill"
        );
    }

    /// D2 — the production WRAPPER frame, not just the derivation it delegates
    /// to.
    ///
    /// `mobile_skill_list_is_derived_from_the_live_registry` above exercises
    /// `register_mobile_skill_commands_from`, so it proves the DERIVATION is
    /// total over the registry it is handed. It cannot see a filter one frame
    /// up, inside `register_mobile_skill_commands` itself — and today no
    /// value-level test can: the bundled registry holds exactly ten skills, so
    /// a literal-free filter that keeps at least ten
    /// (`mobile_skill_registry().names().into_iter().take(10)`, or a
    /// `filter` on a running count) is the identity function right now. It
    /// starts dropping skills only once an eleventh bundled skill lands —
    /// which is precisely the defect P-1.5 removed, silently reintroduced,
    /// invisible to every value assertion in this file and to the
    /// component-literal scanner too, because such a filter names no skill.
    ///
    /// What CAN be pinned today is the shape of that frame: the wrapper must
    /// stay one unconditional delegation of the WHOLE live registry, with
    /// nothing sitting between `mobile_skill_registry()` and the derivation.
    /// Any filter, truncation, `if`, or second name list introduced there
    /// changes this body and fails here, whether or not it mentions a skill
    /// name. (The one frame further up, `register_mobile_bundled_prompt_commands`,
    /// hands the wrapper nothing but `reg`, so it has no registry to filter.)
    #[test]
    fn mobile_skill_command_wrapper_stays_an_unfiltered_delegation() {
        const SOURCE: &str = include_str!("lib.rs");
        // Assembled from fragments so this needle cannot match its own literal
        // in the scanned source — the count assertion below then means the
        // wrapper is defined exactly once and we are reading THAT definition.
        let needle = concat!(
            "pub(crate) fn ",
            "register_mobile_skill_commands",
            "(reg: &mut CommandRegistry) {"
        );
        assert_eq!(
            SOURCE.matches(needle).count(),
            1,
            "expected exactly one definition of the mobile skill-command wrapper"
        );
        let start = SOURCE.find(needle).expect("wrapper definition") + needle.len();
        let mut depth = 1usize;
        let mut end = None;
        for (offset, ch) in SOURCE[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.expect("unbalanced braces while reading the wrapper body");
        let body = SOURCE[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            body, "register_mobile_skill_commands_from(reg, &mobile_skill_registry());",
            "`register_mobile_skill_commands` must remain a single unconditional \
             delegation that hands the derivation the ENTIRE live registry. \
             Anything else in this frame — a `take`/`filter`/`if`, a second name \
             list, a rebuilt registry — can drop a bundled skill from the slash \
             palette while every value assertion in this file still passes, \
             because the registry currently holds exactly ten skills and a \
             filter that keeps ten is today indistinguishable from no filter"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use tool_api::tool_trait::{ToolError, ToolStaticContext};
    use traits::process::ProcessOutput;

    struct NoopSkillLoader;

    #[async_trait]
    impl tool_skill::skill::SkillLoader for NoopSkillLoader {
        async fn load(
            &self,
            _name: &str,
        ) -> Result<Option<tool_skill::skill::SkillDescriptor>, ToolError> {
            Ok(None)
        }
    }

    struct FirstAnswerResolver;

    #[async_trait]
    impl AskUserQuestionResolver for FirstAnswerResolver {
        async fn resolve(
            &self,
            questions: &[tool_ui::ask_user_question::Question],
            _non_interactive: bool,
        ) -> Result<HashMap<String, String>, ToolError> {
            Ok(questions
                .iter()
                .filter_map(|question| {
                    question
                        .options
                        .first()
                        .map(|option| (question.question.clone(), option.label.clone()))
                })
                .collect())
        }
    }

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[cfg(feature = "uniffi")]
    #[tokio::test]
    async fn custom_mobile_ask_resolver_does_not_duplicate_builtin() {
        let registry = mobile_tool_registry_with_skill_loader_and_ask_resolver(
            shell_test_ctx(dummy_out()),
            std::env::temp_dir(),
            Arc::new(NoopSkillLoader),
            Arc::new(FirstAnswerResolver),
        );
        let tools = registry.available_tools(&ToolStaticContext::default());
        let ask_count = tools
            .iter()
            .filter(|tool| tool.name() == tool_ui::ask_user_question::ASK_USER_QUESTION_TOOL_NAME)
            .count();
        assert_eq!(ask_count, 1);
        let ask = registry
            .find_by_name(tool_ui::ask_user_question::ASK_USER_QUESTION_TOOL_NAME)
            .expect("AskUserQuestion registered");
        let result = ask
            .call(
                serde_json::json!({
                    "questions": [{
                        "question": "Pick one?",
                        "header": "Choice",
                        "options": [
                            { "label": "Alpha", "description": "first" },
                            { "label": "Beta", "description": "second" }
                        ]
                    }]
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("custom resolver should answer");
        assert_eq!(result.data["answers"]["Pick one?"].as_str(), Some("Alpha"));
    }

    #[cfg(feature = "uniffi")]
    #[tokio::test]
    async fn mobile_task_create_persists_under_the_app_config_home() {
        let temp = tempfile::tempdir().expect("app container");
        let config_home = temp.path().join(branding::DOT_DIR);
        let registry = mobile_tool_registry_with_skill_loader(
            shell_test_ctx(dummy_out()),
            config_home.clone(),
            Arc::new(NoopSkillLoader),
        );
        let task_create = registry
            .find_by_name(tool_task::task::TASK_CREATE_TOOL_NAME)
            .expect("TaskCreate registered");

        task_create
            .call(
                serde_json::json!({
                    "subject": "Generate local app",
                    "description": "Exercise the iOS app-private task store"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("TaskCreate writes inside the app container");

        let task_files = std::fs::read_dir(config_home.join("tasks"))
            .expect("tasks root created")
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .flat_map(|entry| {
                std::fs::read_dir(entry.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count();
        assert_eq!(task_files, 1, "one task persisted in the injected home");
    }
}
