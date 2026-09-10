import Foundation

enum LocalAppsDistributionMode: String, Sendable {
    case store
    case full

    static var current: Self {
        let value = Bundle.main.object(forInfoDictionaryKey: "LingxiLocalAppsMode") as? String
        if let value, let mode = Self(rawValue: value.lowercased()) { return mode }
        return Bundle.main.bundleIdentifier?.hasSuffix(".full") == true ? .full : .store
    }

    var runtimeLabel: String {
        switch self {
        case .store: String(localized: "local_apps_runtime_static")
        case .full: String(localized: "local_apps_runtime_vite")
        }
    }
}

/// Generic per-app glyph. Every app starts from a brief, so there is no fixed
/// category taxonomy to key an icon off of.
let localAppIconSystemName = "app.badge"

/// The first message the conversational create flow sends on the user's behalf,
/// once the new shell's init session is live.
///
/// Named here rather than written as a literal at the call site so a test can
/// assert the exact string that gets SENT. Nothing in Swift fails to compile
/// over a missing localization key — this branch shipped a call site reading
/// `local_apps_kickoff_message`, a key in no catalog, which would have sent the
/// raw key as the user's first message — so the only way this can be checked at
/// all is for the value to be reachable from a test.
enum LocalAppKickoff {
    /// The catalog key. Shared with Android (`R.string.local_apps_kickoff`), so
    /// the copy is one sentence maintained once for both clients.
    ///
    /// A plain `String` rather than a `String.LocalizationValue` literal so the
    /// test can compare the RESOLVED copy against the key that produced it. A
    /// test comparing against a hardcoded `"local_apps_kickoff"` passes for a
    /// mistyped key — it was written that way first, and a run with the key
    /// deliberately broken passed.
    static let key = "local_apps_kickoff"

    /// The resolved copy. Placeholder-free on purpose: a shell has no brief to
    /// interpolate — finding out what the user wants is the whole job of the
    /// conversation this message opens.
    ///
    /// `String(localized:)` returns the KEY when the key is absent, so an
    /// unresolved key is silent at build time and at run time. The only guard
    /// is `testTheKickoffMessageResolvesToRealCopy`.
    static var message: String {
        String(localized: String.LocalizationValue(stringLiteral: key))
    }
}

struct LocalAppSummary: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
    /// One-line description the user gave at creation time.
    var brief: String
    var gitEnabled: Bool = true
    /// When the record was first written (`AppRecordDto.created_at_ms`).
    ///
    /// Immutable for the life of the app, which is what makes it — and not
    /// `updatedAt` — the honest measure of how long a shell has sat
    /// unscaffolded. `nil` only for a summary this client built itself
    /// (fixtures, the UI-test seed); every record off the wire carries one.
    var createdAt: Date? = nil
    var updatedAt: Date
    var workflow: LocalAppWorkflow
    var workspaceRelativePath: String
    /// The app's pinned "init" session (bare uuid), listed first in its
    /// session catalog. `nil` for pre-v3 records before the boot backfill.
    var initSessionId: String? = nil
    /// Whether the scaffold has landed (protocol 8.0.0's `AppRecordDto.scaffolded`).
    ///
    /// `false` is a SHELL: the record exists and owns a workspace and a pinned
    /// conversation, but `name` is an engine-side placeholder the user never
    /// chose and `brief` is the empty string. Both are decided later, in the
    /// app's own conversation, and only then does `LocalAppScaffold` lay the
    /// scaffold down and flip this to `true`.
    ///
    /// Defaulted to `true` so a fixture or a caller that constructs a summary
    /// without saying anything describes a FORMED app — the shell is the
    /// exceptional state and has to be asked for explicitly.
    var scaffolded: Bool = true

    /// The last host-derived health snapshot for the pinned runtime profile.
    ///
    /// App list records do not carry this detail, so it is populated when a
    /// details snapshot arrives and retained until the next authoritative
    /// details snapshot. `nil` is intentional for shells and for formed apps
    /// whose details have not been loaded yet; the UI must not infer health
    /// from source files or the app's visual/runtime state.
    var runtimeProfileStatus: LocalAppRuntimeProfileStatus? = nil
    /// Host-derived verification summaries remain distinct from publication
    /// state so UI badges do not overload "published" with "verified".
    var uiVerification: LocalAppVerificationSummary? = nil
    var mcpVerification: LocalAppVerificationSummary? = nil

    /// `true` while this app is still an unscaffolded shell.
    ///
    /// The single predicate every render point branches on, so "does this
    /// place hide the placeholder name?" is one question with one answer
    /// rather than five independent `!scaffolded` spellings that can drift.
    var isDraftShell: Bool { !scaffolded }

    /// The title to put on screen.
    ///
    /// A shell's `name` is the engine's placeholder, so showing it would put a
    /// string the user never chose — and cannot act on — at the top of a card.
    var displayName: String {
        isDraftShell ? String(localized: "local_apps_draft_card_title") : name
    }

    /// How long a shell can sit unscaffolded before its card stops claiming
    /// a create is still running.
    ///
    /// Measured from `createdAt`, which is the shell's real age. `updatedAt`
    /// is NOT "last sign of life": the engine writes it twice for a shell —
    /// once in `set_init_session` (seconds after the record appears) and once
    /// in `commit_scaffold`, which stops it being a shell at all. Everything
    /// in between — the kickoff, the whole interview with the user, staging,
    /// scaffolding, dependency install — moves it not at all. So this window
    /// has to clear a realistic conversation, not just a machine step, or a
    /// healthy create gets branded stalled while the user is still typing.
    private static let draftStalledInterval: TimeInterval = 60 * 60

    /// `true` once a still-unscaffolded shell has sat long enough that
    /// "Creating…" would be a lie: the create failed, or this client never
    /// heard back, and nothing else marks that state anywhere in the UI.
    ///
    /// Anchored on `createdAt` rather than `updatedAt` because `updatedAt`
    /// can still be re-stamped after the create it describes is already dead.
    /// The precondition is NARROW, and worth stating exactly so nobody
    /// "corrects" this back: `set_init_session` is set-once — only its
    /// `None` arm writes `updated_at_ms`, and a record that already carries a
    /// pin is refused with `InvalidRequest` (`local-apps/src/service.rs`) —
    /// and the boot sweep `continue`s past every already-pinned record before
    /// it would mint one (`apps/engine-mobile/src/host.rs`). So this is NOT a
    /// bump on every launch. It is at most ONE bump, and only for a shell
    /// whose create died before it could pin: on the next launch the backfill
    /// pins it, `updatedAt` jumps to now, and a window anchored there restarts
    /// — the card goes back to claiming a create is in flight for another hour
    /// on a create that has been dead for days. `createdAt` cannot be moved at
    /// all, which is why it is the anchor even for that one bump.
    /// Falls back to `updatedAt` when the summary was built without one.
    var isDraftStalled: Bool {
        isDraftShell
            && Date().timeIntervalSince(createdAt ?? updatedAt) > Self.draftStalledInterval
    }

    /// The secondary line a shell replaces its normal status line with, or
    /// `nil` once the app is formed and its own status applies.
    ///
    /// Returned as an Optional rather than a plain String so each call site
    /// reads `app.draftStatusLine ?? <its own status>` and cannot forget the
    /// shell case by writing only its own branch.
    ///
    /// The stalled copy is deliberately NOT a terminal failure
    /// (`local_apps_workflow_generation_failed`, "Generation Failed"):
    /// nothing here observed a failure. All that is known is that the shell
    /// never scaffolded within the window — the create may have died, or the
    /// interview may simply have been abandoned half-way and be resumable.
    /// "Setup unfinished" is what is actually true of both.
    var draftStatusLine: String? {
        guard isDraftShell else { return nil }
        return isDraftStalled
            ? String(localized: "local_apps_draft_card_subtitle_stalled")
            : String(localized: "local_apps_draft_card_subtitle")
    }

    /// The brief, or `nil` while this app is a shell.
    ///
    /// A shell's brief is `""` — the engine writes no placeholder for it —
    /// so rendering it produces an empty labelled row, which reads as a bug.
    var displayBrief: String? { isDraftShell ? nil : brief }
}

enum LocalAppLaunchDestination: String, Hashable, Sendable {
    case details
    case preview
}

struct LocalAppStatusBadge: Hashable, Sendable {
    let label: String
    let accessibilityLabel: String
    let systemImageName: String
    let tintName: String
}

struct LocalAppBuiltinPluginDescriptor: Hashable, Sendable {
    let pluginID: String
    let displayName: String
    let version: String
    let archiveDigest: String
    let skillCount: Int
    let agentCount: Int
    let workflowCount: Int
    let templateCount: Int
    let defaultEnabled: Bool

    static let current = LocalAppBuiltinPluginDescriptor(
        pluginID: "lingxi-local-app",
        displayName: "LingXi Local App",
        version: "1.0.0",
        // Intentionally blank, NOT a hand-typed hash: this constant is only
        // ever shown before the live `pluginInventoryChanged` event arrives
        // (`LocalAppsStore.swift` overwrites `archiveDigest` with the
        // engine-computed `bundleDigest` the moment it does), so a literal
        // here can only ever be a snapshot of some past build's real digest
        // — one that silently drifts as the plugin bundle changes and, once
        // wrong, reads as an authoritative hash while claiming nothing true.
        // `digestSummary` (Settings/MCPPages.swift) renders an empty string
        // as an empty value rather than a plausible-looking wrong one.
        archiveDigest: "",
        skillCount: 27,
        agentCount: 7,
        workflowCount: 3,
        templateCount: 5,
        defaultEnabled: true
    )
}

struct LocalAppBuiltinPluginStatus: Hashable, Sendable {
    let state: PluginActivationStateDto
    let manifestDefaultEnabled: Bool
    let validationError: String?

    var isEnabled: Bool { state == .loaded }

    var statusBadge: LocalAppStatusBadge {
        if isEnabled {
            return LocalAppStatusBadge(
                label: String(localized: "settings_status_on"),
                accessibilityLabel: String(localized: "local_apps_plugin_enabled"),
                systemImageName: "checkmark.circle.fill",
                tintName: "green"
            )
        }
        return LocalAppStatusBadge(
            label: String(localized: "settings_status_off"),
            accessibilityLabel: String(localized: "local_apps_plugin_disabled"),
            systemImageName: "pause.circle.fill",
            tintName: "secondary"
        )
    }
}

extension PluginActivationStateDto: @unchecked Sendable {}

struct LocalAppBuiltinPluginInventory: Hashable, Sendable {
    let pluginID: String
    let displayName: String
    let source: String
    let version: String
    let bundleDigest: String
    let manifestDefaultEnabled: Bool
    let skillCount: Int
    let agentCount: Int
    let workflowCount: Int
    let templateCount: Int
    let validationError: String?
}

enum LocalAppVerificationStatus: String, CaseIterable, Hashable, Sendable {
    case pending
    case passed
    case failed
    case unverified
    case unavailable

    var badge: LocalAppStatusBadge {
        switch self {
        case .pending:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_status_pending"),
                accessibilityLabel: String(localized: "local_apps_verification_status_pending"),
                systemImageName: "clock.fill",
                tintName: "orange"
            )
        case .passed:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_status_passed"),
                accessibilityLabel: String(localized: "local_apps_verification_status_passed"),
                systemImageName: "checkmark.circle.fill",
                tintName: "green"
            )
        case .failed:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_status_failed"),
                accessibilityLabel: String(localized: "local_apps_verification_status_failed"),
                systemImageName: "xmark.octagon.fill",
                tintName: "orange"
            )
        case .unverified:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_status_unverified"),
                accessibilityLabel: String(localized: "local_apps_verification_status_unverified"),
                systemImageName: "questionmark.circle.fill",
                tintName: "secondary"
            )
        case .unavailable:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_status_unavailable"),
                accessibilityLabel: String(localized: "local_apps_verification_status_unavailable"),
                systemImageName: "slash.circle.fill",
                tintName: "secondary"
            )
        }
    }
}

struct LocalAppVerificationSummary: Hashable, Sendable {
    let status: LocalAppVerificationStatus
    /// The engine's own sentence, always English: `local_apps_host.rs` builds
    /// it from string literals and has no notion of the client's locale.
    /// Kept verbatim as the fallback for a `code` this build does not know.
    let summary: String
    /// The stable machine code the engine sends alongside `summary`
    /// (`LocalAppVerificationSummaryDto.code`) precisely so that a client can
    /// say the same thing out of its own catalog.
    let code: String?
    /// `false` for a summary this client fabricated rather than decoded off
    /// the wire (`LocalAppManagedMcpInventoryReader`, which reads the on-disk
    /// manifest when the engine has not emitted an inventory yet).
    ///
    /// It exists because "no code" is MEANINGFUL on the wire — the engine
    /// omits `code` for exactly one state, a clean MCP pass — and a
    /// client-built summary that happens to be `passed` with no code means
    /// nothing of the sort. Without this, the reader's own
    /// "Published UI verification passed." rendered as the MCP pass sentence.
    var isHostSourced: Bool = true

    var badge: LocalAppStatusBadge { status.badge }

    /// The verification sentence to put on screen.
    ///
    /// The four values `code` can take are the four production emitters in
    /// `apps/engine-mobile/src/local_apps_host.rs`: `needs_setup`,
    /// `needs_revalidation`, `verification_unavailable`, and NO code at all
    /// for a clean pass. The `nil` case is the one that matters most — it is
    /// the state that otherwise still renders the engine's English.
    ///
    /// An unrecognized FUTURE code falls back to the engine's raw sentence
    /// rather than showing nothing, the same rule `localizedGateLabel`
    /// (`LocalAppApprovalSheets.swift`) follows for gate ids.
    ///
    /// `nil` is only read as "passed" when the status agrees AND the summary
    /// came off the wire. A summary this client synthesizes carries no code by
    /// design (see `updateManagedInventoryFailure` and
    /// `LocalAppManagedMcpInventoryReader`), and must keep rendering its own
    /// message rather than claiming MCP verification passed.
    var localizedSummary: String {
        switch code {
        case "needs_setup":
            String(localized: "local_apps_verification_summary_needs_setup")
        case "needs_revalidation":
            String(localized: "local_apps_verification_summary_needs_revalidation")
        case "verification_unavailable":
            String(localized: "local_apps_verification_summary_verification_unavailable")
        case "ui_verification_required":
            String(localized: "local_apps_verification_summary_ui_verification_required")
        case "ui_verification_passed":
            String(localized: "local_apps_verification_summary_ui_verification_passed")
        case "ui_verification_corrupt":
            String(localized: "local_apps_verification_summary_ui_verification_corrupt")
        case nil where isHostSourced && status == .passed:
            String(localized: "local_apps_verification_summary_passed")
        default:
            summary
        }
    }
}

struct LocalAppGateStatus: Identifiable, Hashable, Sendable {
    let gateID: String
    let label: String
    let status: LocalAppVerificationStatus
    let available: Bool
    let detail: String?

    var id: String { gateID }
    var badge: LocalAppStatusBadge { status.badge }
}

struct LocalAppTemplateSummary: Hashable, Sendable {
    let templateID: String
    let surface: LocalAppRuntimeProfileSurface
    let summary: String
}

struct LocalAppRejectedCandidate: Identifiable, Hashable, Sendable {
    let templateID: String
    let reason: String

    var id: String { templateID }
}

struct LocalAppMcpToolSurface: Identifiable, Hashable, Sendable {
    let name: String
    let title: String?
    let description: String?
    let inputSchemaSummary: String
    let outputSchemaSummary: String?
    let annotationsSummary: String?
    let executionSummary: String?
    let visibleMetaSummary: String?
    let semanticFlowSummary: String
    let ceilingSummary: String

    var id: String { name }
}

typealias LocalAppManagedMcpTool = LocalAppMcpToolSurface

enum LocalAppMcpToolField: String, CaseIterable, Hashable, Sendable {
    case name
    case title
    case description
    case inputSchema
    case outputSchema
    case annotations
    case execution
    case visibleMeta
    case semanticFlow
    case permissionCeiling

    var label: String {
        switch self {
        case .name:
            String(localized: "settings_display_name")
        case .title:
            "Title"
        case .description:
            "Description"
        case .inputSchema:
            String(localized: "local_apps_mcp_proposal_field_input_schema")
        case .outputSchema:
            String(localized: "local_apps_mcp_proposal_field_output_schema")
        case .annotations:
            String(localized: "local_apps_mcp_proposal_field_annotations")
        case .execution:
            String(localized: "local_apps_mcp_proposal_field_execution")
        case .visibleMeta:
            String(localized: "local_apps_mcp_proposal_field_visible_meta")
        case .semanticFlow:
            String(localized: "local_apps_mcp_proposal_field_semantic_flow")
        case .permissionCeiling:
            String(localized: "local_apps_mcp_proposal_field_permission_ceiling")
        }
    }
}

enum LocalAppMcpToolChangeKind: String, CaseIterable, Hashable, Sendable {
    case added
    case removed
    case changed

    var label: String {
        switch self {
        case .added:
            String(localized: "local_apps_mcp_proposal_added")
        case .removed:
            String(localized: "local_apps_mcp_proposal_removed")
        case .changed:
            String(localized: "local_apps_mcp_proposal_changed")
        }
    }
}

struct LocalAppMcpToolDiff: Identifiable, Hashable, Sendable {
    let kind: LocalAppMcpToolChangeKind
    let name: String
    let before: LocalAppMcpToolSurface?
    let after: LocalAppMcpToolSurface?
    let changedFields: [LocalAppMcpToolField]

    var id: String { "\(kind.rawValue)-\(name)" }
}

struct LocalAppCreateConfirmationPrompt: Identifiable, Hashable, Sendable {
    let requestID: String
    let appID: String
    let name: String
    let brief: String
    let selectedTemplate: LocalAppTemplateSummary
    let runtimeProfile: LocalAppRuntimeProfileOption
    let reason: String
    let rejected: [LocalAppRejectedCandidate]
    let initialTools: [LocalAppMcpToolSurface]
    let requiredGates: [LocalAppGateStatus]

    var id: String { requestID }
}

struct LocalAppMcpProposalApprovalPrompt: Identifiable, Hashable, Sendable {
    let requestID: String
    let appID: String
    let workflowRunID: String
    let summary: String
    let proposalSHA256: String
    let approvalContractSHA256: String
    let toolSurfaceSHA256: String
    let toolDiffs: [LocalAppMcpToolDiff]
    let requiredFlowChanges: [String]
    let excludedCapabilities: [String]
    let pendingGates: [LocalAppGateStatus]

    var id: String { requestID }
    var hasVisibleChanges: Bool {
        !toolDiffs.isEmpty || !requiredFlowChanges.isEmpty || !excludedCapabilities.isEmpty || !pendingGates.isEmpty
    }
}

struct LocalAppManagedMcpInventory: Identifiable, Hashable, Sendable {
    let serverName: String
    let appID: String
    let appName: String
    let buildID: String
    let catalogDigest: String
    let toolSurfaceDigest: String
    let authoringRevision: UInt64
    let enabled: Bool
    let status: LocalAppManagedMcpStatus
    let settingsRevision: UInt64
    let pinnedToCurrentConversation: Bool
    let publicationState: LocalAppWorkflow
    let mcpVerification: LocalAppVerificationSummary
    let uiVerification: LocalAppVerificationSummary
    let enabledTools: Set<String>
    let widget: LocalAppManagedMcpWidget?
    let tools: [LocalAppManagedMcpTool]

    var id: String { serverName }
    var toolCount: Int { tools.count }
    var publicationBadge: LocalAppStatusBadge { publicationState.statusBadge }
    var statusBadge: LocalAppStatusBadge { status.badge }

    func isToolEnabled(_ toolName: String) -> Bool {
        enabledTools.contains(toolName)
    }

    func updatingEnabled(_ value: Bool) -> Self {
        Self(
            serverName: serverName,
            appID: appID,
            appName: appName,
            buildID: buildID,
            catalogDigest: catalogDigest,
            toolSurfaceDigest: toolSurfaceDigest,
            authoringRevision: authoringRevision,
            enabled: value,
            status: value ? .enabled : .disabled,
            settingsRevision: settingsRevision,
            pinnedToCurrentConversation: pinnedToCurrentConversation,
            publicationState: publicationState,
            mcpVerification: mcpVerification,
            uiVerification: uiVerification,
            enabledTools: enabledTools,
            widget: widget,
            tools: tools
        )
    }

    func updatingTool(_ toolName: String, enabled value: Bool) -> Self {
        var nextEnabledTools = enabledTools
        if value {
            nextEnabledTools.insert(toolName)
        } else {
            nextEnabledTools.remove(toolName)
        }
        return Self(
            serverName: serverName,
            appID: appID,
            appName: appName,
            buildID: buildID,
            catalogDigest: catalogDigest,
            toolSurfaceDigest: toolSurfaceDigest,
            authoringRevision: authoringRevision,
            enabled: enabled,
            status: enabled ? .enabled : .disabled,
            settingsRevision: settingsRevision,
            pinnedToCurrentConversation: pinnedToCurrentConversation,
            publicationState: publicationState,
            mcpVerification: mcpVerification,
            uiVerification: uiVerification,
            enabledTools: nextEnabledTools,
            widget: widget,
            tools: tools
        )
    }

    func updatingConversationPinned(_ value: Bool) -> Self {
        Self(
            serverName: serverName,
            appID: appID,
            appName: appName,
            buildID: buildID,
            catalogDigest: catalogDigest,
            toolSurfaceDigest: toolSurfaceDigest,
            authoringRevision: authoringRevision,
            enabled: enabled,
            status: status,
            settingsRevision: settingsRevision,
            pinnedToCurrentConversation: value,
            publicationState: publicationState,
            mcpVerification: mcpVerification,
            uiVerification: uiVerification,
            enabledTools: enabledTools,
            widget: widget,
            tools: tools
        )
    }

    static func placeholder(
        appID: String,
        appName: String,
        publicationState: LocalAppWorkflow = .draft
    ) -> Self {
        Self(
            serverName: "local_app_\(appID)",
            appID: appID,
            appName: appName,
            buildID: "",
            catalogDigest: "",
            toolSurfaceDigest: "",
            authoringRevision: 0,
            enabled: false,
            status: .needsSetup,
            settingsRevision: 0,
            pinnedToCurrentConversation: false,
            publicationState: publicationState,
            // Client-built placeholders, not decoded from the wire — see
            // `LocalAppVerificationSummary.isHostSourced`.
            mcpVerification: LocalAppVerificationSummary(
                status: .unavailable,
                summary: "No MCP surface has been authored for this app yet.",
                code: nil,
                isHostSourced: false
            ),
            uiVerification: LocalAppVerificationSummary(
                status: .unavailable,
                summary: "No MCP widget is available yet.",
                code: nil,
                isHostSourced: false
            ),
            enabledTools: [],
            widget: nil,
            tools: []
        )
    }
}

struct LocalAppManagedMcpWidget: Hashable, Sendable {
    let title: String?
    let resourceURI: String?
    let mimeType: String?
}

enum LocalAppManagedMcpStatus: String, CaseIterable, Hashable, Sendable {
    case disabled
    case needsSetup = "needs_setup"
    case authoring
    case enabled
    case needsRevalidation = "needs_revalidation"
    case error

    var title: String {
        switch self {
        case .disabled:
            "Disabled"
        case .needsSetup:
            "Needs setup"
        case .authoring:
            "Authoring"
        case .enabled:
            "Enabled"
        case .needsRevalidation:
            "Needs revalidation"
        case .error:
            "Error"
        }
    }

    var summary: String {
        switch self {
        case .disabled:
            "This app's MCP surface is registered but currently off."
        case .needsSetup:
            "This app does not have an MCP surface yet. Describe what the assistant should be allowed to do, then start authoring."
        case .authoring:
            "An MCP authoring flow is in progress for this app."
        case .enabled:
            "The assistant can call this app through its managed MCP surface."
        case .needsRevalidation:
            "The stored MCP surface needs to be reviewed before it is exposed again."
        case .error:
            "The last MCP operation failed. Review the error and retry."
        }
    }

    var badge: LocalAppStatusBadge {
        switch self {
        case .disabled:
            LocalAppStatusBadge(
                label: title,
                accessibilityLabel: title,
                systemImageName: "pause.circle.fill",
                tintName: "secondary"
            )
        case .needsSetup:
            LocalAppStatusBadge(
                label: title,
                accessibilityLabel: title,
                systemImageName: "wrench.and.screwdriver.fill",
                tintName: "orange"
            )
        case .authoring:
            LocalAppStatusBadge(
                label: title,
                accessibilityLabel: title,
                systemImageName: "wand.and.stars",
                tintName: "orange"
            )
        case .enabled:
            LocalAppStatusBadge(
                label: title,
                accessibilityLabel: title,
                systemImageName: "checkmark.circle.fill",
                tintName: "green"
            )
        case .needsRevalidation:
            LocalAppStatusBadge(
                label: title,
                accessibilityLabel: title,
                systemImageName: "arrow.triangle.2.circlepath",
                tintName: "orange"
            )
        case .error:
            LocalAppStatusBadge(
                label: title,
                accessibilityLabel: title,
                systemImageName: "xmark.octagon.fill",
                tintName: "orange"
            )
        }
    }
}

enum LocalAppManagedMcpCommand: Hashable, Sendable {
    case startAuthoring(appID: String, userGoal: String)
    case setEnabled(appID: String, enabled: Bool, expectedRevision: UInt64)
    case setToolEnabled(appID: String, toolName: String, enabled: Bool, expectedRevision: UInt64)
    case setConversationPinned(conversationID: String, appID: String, pinned: Bool)
}

/// Workflow state of an app — the v3 publication projection. The old
/// designer/plan/generation pipeline is gone; clients render the trusted
/// publication pair plus verification state instead.
enum LocalAppWorkflow: String, CaseIterable, Hashable, Sendable {
    case draft
    case publishedUnverified = "published_unverified"
    case publishedVerified = "published_verified"

    var label: String {
        switch self {
        case .draft: String(localized: "local_apps_state_draft")
        case .publishedUnverified: String(localized: "local_apps_verification_published_unverified")
        case .publishedVerified: String(localized: "local_apps_verification_published_verified")
        }
    }

    var isPublished: Bool {
        switch self {
        case .draft:
            false
        case .publishedUnverified, .publishedVerified:
            true
        }
    }

    var statusBadge: LocalAppStatusBadge {
        switch self {
        case .draft:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_state_draft"),
                accessibilityLabel: String(localized: "local_apps_verification_draft"),
                systemImageName: "pencil.circle.fill",
                tintName: "secondary"
            )
        case .publishedUnverified:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_published_unverified"),
                accessibilityLabel: String(localized: "local_apps_verification_published_unverified"),
                systemImageName: "exclamationmark.triangle.fill",
                tintName: "orange"
            )
        case .publishedVerified:
            LocalAppStatusBadge(
                label: String(localized: "local_apps_verification_published_verified"),
                accessibilityLabel: String(localized: "local_apps_verification_published_verified"),
                systemImageName: "checkmark.seal.fill",
                tintName: "green"
            )
        }
    }
}

/// One row of an app's workspace-scoped session catalog — the UI projection of
/// the wire `AppSessionRowDto`. `isInit` marks the pinned init session
/// (`AppSessionKindDto.init`), listed first regardless of modified order.
struct LocalAppSessionRow: Identifiable, Hashable, Sendable {
    /// Bare session uuid — also the resume key.
    let uuid: String
    let title: String
    let mode: SessionMode
    let modifiedAt: Date?
    /// Relative display form of the wire's RFC 3339 `modified` stamp.
    let relativeTime: String
    let messageCount: Int
    let isInit: Bool

    var id: String { uuid }
}

/// One requested page of an app's session catalog plus the paging cursor
/// (`nextOffset == nil` on the last page).
struct LocalAppSessionPage: Hashable, Sendable {
    var rows: [LocalAppSessionRow] = []
    var nextOffset: UInt64?
}

struct LocalAppProfileProposal: Identifiable, Equatable, Sendable {
    let appID: String
    let approvalToken: String
    let baseRevision: UInt64
    let currentRevision: UInt64
    let instructions: String
    let reason: String

    var id: String { approvalToken }
}

enum LocalAppDataFieldType: String, CaseIterable, Hashable, Sendable {
    case text
    case longText = "long_text"
    case integer
    case decimal
    case boolean
    case dateTime = "date_time"
    case enumeration = "enum"
    case imageReference = "image_ref"
}

struct LocalAppDataField: Identifiable, Hashable, Sendable {
    var id: String
    var label: String
    var fieldType: LocalAppDataFieldType
    var required: Bool
    var options: [String]
}

struct LocalAppDataCollection: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let fields: [LocalAppDataField]
    let enabledByDefault: Bool
}

enum LocalAppRuntimeStatus: Hashable, Sendable {
    case stopped
    case starting
    case running(URL?)
    case suspended(String?)
    case stopping
    case failed(String)

    var label: String {
        switch self {
        case .stopped: String(localized: "local_apps_runtime_stopped")
        case .starting: String(localized: "local_apps_runtime_starting")
        case .running: String(localized: "local_apps_runtime_running")
        case let .suspended(reason): reason.map { String(localized: "local_apps_runtime_suspended \($0)") } ?? String(localized: "local_apps_runtime_suspended_plain")
        case .stopping: String(localized: "local_apps_runtime_stopping")
        case let .failed(reason): String(localized: "local_apps_runtime_failed \(reason)")
        }
    }

    var url: URL? {
        guard case let .running(url) = self else { return nil }
        return url
    }
}

/// What the pushed preview route shows when there is no live URL to load.
///
/// Extracted from the view so the mapping is testable. The route used to
/// collapse `.starting`, `.stopped` and `.failed(reason)` into a single
/// hourglass with no action and no reason, which made a runtime that had
/// FAILED indistinguishable from one that was merely still booting — and left
/// the user no way to retry.
enum LocalAppPreviewPlaceholder: Equatable, Sendable {
    /// A transition is in flight; the URL should arrive on its own.
    case transient
    /// The runtime failed and will not come up without action.
    case failed(String)
    /// The host suspended the runtime.
    case suspended(String?)
    /// Nothing is running and nothing is in flight.
    case idle

    /// `nil` means "there is a URL — render the web view instead".
    static func forStatus(_ status: LocalAppRuntimeStatus?) -> LocalAppPreviewPlaceholder? {
        switch status {
        case let .running(url):
            // `running` without a URL yet is still coming up, not idle.
            url == nil ? .transient : nil
        case .starting, .stopping:
            .transient
        case let .failed(reason):
            .failed(reason)
        case let .suspended(reason):
            .suspended(reason)
        case .stopped, nil:
            .idle
        }
    }
}

struct LocalAppCheckpoint: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let kind: String
    let createdAt: Date
}

enum LocalAppCapabilityDecision: String, CaseIterable, Identifiable, Sendable {
    case once
    case session
    case always
    case deny

    var id: String { rawValue }

    var label: String {
        switch self {
        case .once: String(localized: "local_apps_allow_once")
        case .session: String(localized: "local_apps_allow_session_short")
        case .always: String(localized: "local_apps_allow_always")
        case .deny: String(localized: "local_apps_deny")
        }
    }
}

struct LocalAppPermissionPrompt: Identifiable, Hashable, Sendable {
    enum Kind: Hashable, Sendable {
        case dataMutation
        case uiControl
        case networkDomain
        case restoreCheckpoint
        case dependencyChange
        case camera
        case photoLibrary
        case microphone
        case location
        case notifications
        case clipboard
        case share
        case textToSpeech
        case files
        case filesRead
        case filesWrite
        case deviceStatus
        case haptics
        case deepLink
        case calendar
        case contacts
        case media
        case llm
        case agentNotify
        case backgroundSchedule
        case uiAction(String)
    }

    let id: String
    let appID: String
    let kind: Kind
    let reason: String
    let domain: String?

    var allowsPersistentGrant: Bool {
        switch kind {
        case .dependencyChange:
            false
        default:
            true
        }
    }

    var title: String {
        switch kind {
        case .dataMutation: String(localized: "local_apps_permission_data_mutation")
        case .uiControl: String(localized: "local_apps_permission_ui_control")
        case .networkDomain: String(localized: "local_apps_permission_network")
        case .restoreCheckpoint: String(localized: "local_apps_permission_restore")
        case .dependencyChange: String(localized: "local_apps_permission_dependency_change")
        case .camera: String(localized: "local_apps_permission_camera")
        case .photoLibrary: String(localized: "local_apps_permission_photo_library")
        case .microphone: String(localized: "local_apps_permission_microphone")
        case .location: String(localized: "local_apps_permission_location")
        case .notifications: String(localized: "local_apps_permission_notifications")
        case .clipboard: "允许应用读取或写入系统剪贴板？"
        case .share: "允许应用打开系统分享面板？"
        case .textToSpeech: "允许应用将文字转换为语音？"
        case .filesRead:
            "允许应用读取自己的私有文件？"
        case .filesWrite:
            "允许应用写入自己的私有文件？"
        case .files:
            if reason.contains("写入") {
                "允许应用写入自己的私有文件？"
            } else if reason.contains("读取") {
                "允许应用读取自己的私有文件？"
            } else {
                "允许应用访问自己的私有文件？"
            }
        case .deviceStatus: "允许应用读取设备状态？"
        case .haptics: "允许应用触发触觉反馈？"
        case .deepLink: "允许应用打开外部链接？"
        case .calendar: "允许应用读取指定范围内的日历事件？"
        case .contacts: "允许应用搜索联系人？"
        case .media: "允许应用读取自己刚获取的媒体？"
        case .llm: String(localized: "local_apps_permission_llm")
        case .agentNotify: String(localized: "local_apps_permission_agent_notify")
        case .backgroundSchedule: "允许应用在系统后台按计划运行流程？"
        case let .uiAction(action): String(localized: "local_apps_permission_ui_action \(action)")
        }
    }
}

enum LocalAppRuntimeProfileFamily: String, CaseIterable, Identifiable, Hashable, Sendable {
    case reactDom
    case canvas2d
    case three3d
    case phaser2d
    case babylon3d

    var id: String { rawValue }

    var title: String {
        switch self {
        case .reactDom: String(localized: "local_apps_runtime_profile_family_react_dom")
        case .canvas2d: String(localized: "local_apps_runtime_profile_family_canvas_2d")
        case .three3d: String(localized: "local_apps_runtime_profile_family_three_3d")
        case .phaser2d: String(localized: "local_apps_runtime_profile_family_phaser_2d")
        case .babylon3d: String(localized: "local_apps_runtime_profile_family_babylon_3d")
        }
    }
}

enum LocalAppRuntimeProfileSurface: Hashable, Sendable {
    case dom
    case canvas

    var title: String {
        switch self {
        case .dom: String(localized: "local_apps_runtime_profile_surface_dom")
        case .canvas: String(localized: "local_apps_runtime_profile_surface_canvas")
        }
    }
}

/// Host-derived health of one app's pinned runtime profile. The raw values
/// are the protocol wire values; keeping them stable lets cache-only cards and
/// detail views share the same finite status vocabulary without parsing host
/// error prose.
enum LocalAppRuntimeProfileStatus: String, CaseIterable, Hashable, Sendable {
    case verified
    case dependenciesDirty = "dependencies_dirty"
    case coreDependencyDrift = "core_dependency_drift"
    case rebuildRequired = "rebuild_required"
    case migrationAvailable = "migration_available"
    case runtimeBundleMissing = "runtime_bundle_missing"
    case runtimeContractCorrupt = "runtime_contract_corrupt"

    var title: String {
        switch self {
        case .verified: String(localized: "local_apps_runtime_profile_health_verified")
        case .dependenciesDirty: String(localized: "local_apps_runtime_profile_health_dependencies_dirty")
        case .coreDependencyDrift: String(localized: "local_apps_runtime_profile_health_core_dependency_drift")
        case .rebuildRequired: String(localized: "local_apps_runtime_profile_health_rebuild_required")
        case .migrationAvailable: String(localized: "local_apps_runtime_profile_health_migration_available")
        case .runtimeBundleMissing: String(localized: "local_apps_runtime_profile_health_runtime_bundle_missing")
        case .runtimeContractCorrupt: String(localized: "local_apps_runtime_profile_health_runtime_contract_corrupt")
        }
    }

    var systemImageName: String {
        switch self {
        case .verified: "checkmark.seal.fill"
        case .dependenciesDirty, .migrationAvailable: "arrow.triangle.2.circlepath"
        case .coreDependencyDrift, .runtimeContractCorrupt: "exclamationmark.shield.fill"
        case .rebuildRequired: "hammer.fill"
        case .runtimeBundleMissing: "shippingbox.fill"
        }
    }
}

struct LocalAppRuntimeProfilePackage: Hashable, Sendable {
    let name: String
    let version: String
}

struct LocalAppRuntimeProfileOption: Identifiable, Hashable, Sendable {
    let family: LocalAppRuntimeProfileFamily
    let revision: UInt32
    let contractSHA256: String
    let surface: LocalAppRuntimeProfileSurface
    let corePackages: [LocalAppRuntimeProfilePackage]
    let cacheStatus: String
    let downloadStatus: String
    let available: Bool
    let reason: String?

    var id: String { family.rawValue }
}

enum LocalAppDependencyChangeKind: String, Hashable, Sendable {
    case add
    case update
    case remove

    var title: String {
        switch self {
        case .add: String(localized: "local_apps_dependency_change_add")
        case .update: String(localized: "local_apps_dependency_change_update")
        case .remove: String(localized: "local_apps_dependency_change_remove")
        }
    }
}

struct LocalAppDependencyChange: Hashable, Sendable {
    let kind: LocalAppDependencyChangeKind
    let package: String
    let version: String?
    let cacheStatus: String
    let downloadStatus: String
}

struct LocalAppDependencyChangeConfirmationPrompt: Identifiable, Hashable, Sendable {
    let id: String
    let appID: String
    let reason: String
    let changes: [LocalAppDependencyChange]
    let licenseRisk: String
    let sbomRisk: String
    let lifecycleScriptsBlocked: Bool
    let nativeAddonsBlocked: Bool
    let rollbackPolicy: String
}
