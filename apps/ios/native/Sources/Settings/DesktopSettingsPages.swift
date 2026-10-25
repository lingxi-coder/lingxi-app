import SwiftUI

struct SettingsConnectionStatus: View {
    @State private var repository = DesktopSettingsRepository.shared
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !repository.connected {
                Label("settings_parity_disconnected", systemImage: "network.slash").accessibilityIdentifier("settings.engine-disconnected")
            } else if !repository.loaded {
                ProgressView("settings_parity_wait_snapshot")
                Button(String(localized: "common_retry")) { Task { await repository.refresh() } }
            }
            if let error = repository.errorMessage { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            if let status = repository.statusMessage { Text(status).foregroundStyle(.secondary) }
        }.font(.footnote).frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Edits one key from the selected file's own snapshot. Never uses effective
/// merged values as a draft, because updateSettings replaces entire keys.
struct LayeredSettingsPage: View {
    let keys: [String]
    @State private var layer = DesktopSettingsLayer.user
    @State private var repository = DesktopSettingsRepository.shared
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsConnectionStatus()
            SettingsLayerPicker(selection: $layer)
            Text(layer == .managed ? String(localized: "settings_parity_managed_readonly") : String(localized: "settings_parity_layer_explanation"))
                .font(.caption).foregroundStyle(.secondary)
            ForEach(keys, id: \.self) { key in
                SettingsValueEditor(key: key, layer: layer)
            }
        }
    }
}

struct SettingsValueEditor: View {
    let key: String
    let layer: DesktopSettingsLayer
    @State private var repository = DesktopSettingsRepository.shared
    @State private var draft = "null"
    @State private var dirty = false
    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                Text(repository.provenanceLabel(for: key)).font(.caption).foregroundStyle(.secondary)
                if repository.locked.contains(key) { Label("settings_parity_managed_readonly", systemImage: "lock") }
                TextEditor(text: $draft)
                    .font(.system(.body, design: .monospaced))
                    .frame(minHeight: 100, maxHeight: 240)
                    .disabled(!repository.canEdit(key: key, layer: layer))
                    .autocorrectionDisabled().textInputAutocapitalization(.never)
                    .onChange(of: draft) { _, _ in dirty = draft != ownJSON }
                HStack {
                    Button("settings_save") { Task { await repository.save(key: key, json: draft, layer: layer) } }
                        .disabled(!dirty || !repository.canEdit(key: key, layer: layer))
                    Button("settings_parity_discard_draft") { reload() }.disabled(!dirty)
                }
                DisclosureGroup("settings_parity_effective_value") {
                    Text(DesktopSettingsRepository.json(repository.effective[key] ?? NSNull()))
                        .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                }
                Text(String(localized: "settings_json_value_hint")).font(.caption2).foregroundStyle(.secondary)
            }
        } label: { Text(key) }
        .onAppear { reload() }
        .onChange(of: layer) { _, _ in reload() }
        .onChange(of: repository.sourceGeneration) { _, _ in reload() }
        .onChange(of: repository.revision) { _, _ in if !dirty || draft == ownJSON { reload() } }
    }
    private var ownJSON: String { DesktopSettingsRepository.json(repository.ownValue(key: key, layer: layer) ?? NSNull()) }
    private func reload() { draft = ownJSON; dirty = false }
}

struct DesktopGeneralPage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    var body: some View {
        VStack(spacing: 12) {
            SettingsSection(label: String(localized: "settings_preferences")) {
                SettingsRow(label: String(localized: "settings_language_title"), onTap: { host.push(.language) })
                SettingsRow(label: String(localized: "settings_notifications"), onTap: { host.push(.notifications) })
                SettingsRow(label: String(localized: "settings_keyboard_input"), onTap: { host.push(.input) })
                SettingsRow(label: String(localized: "settings_app_integration"), isLast: true, onTap: { host.push(.appIntegration) })
                    .accessibilityIdentifier("settings.appIntegration")
            }
            SettingsSection(label: String(localized: "settings_parity_mobile_capabilities")) {
                SettingsRow(label: String(localized: "settings_linux_runtime"), onTap: { host.push(.linuxRuntime) })
                SettingsRow(label: String(localized: "settings_dream_mode"), onTap: { host.push(.dream) })
                SettingsRow(label: String(localized: "settings_typescript_lsp_title"), onTap: { host.push(.typescriptLsp) })
                SettingsRow(label: String(localized: "settings_data_privacy"), isLast: true, onTap: { host.push(.privacy) })
            }
        }
    }
}

struct DesktopAccountPage: View {
    let host: SettingsHost
    var body: some View { LingxiAccountSection() }
}

struct DesktopDiagnosticsPage: View {
    @State private var repository = DesktopSettingsRepository.shared
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            Button("settings_parity_refresh_engine") { Task { await repository.refresh() } }.disabled(!repository.connected)
            if let report = repository.doctor {
                ForEach(Array(report.checks.enumerated()), id: \.offset) { _, check in
                    LabeledContent(check.name, value: String(describing: check.status))
                }
            }
            DisclosureGroup("settings_parity_configuration_files") {
                Text(repository.filesJSON).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
            ShareLink(item: repository.filesJSON) { Label(String(localized: "settings_diagnostics_export"), systemImage: "square.and.arrow.up") }
                .disabled(!repository.loaded)
        }
    }
}

struct DesktopAboutPage: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String ?? "LingXi").font(.title2.bold())
            LabeledContent(String(localized: "plugins_detail_version"), value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—")
            LabeledContent(String(localized: "settings_build_number"), value: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—")
            Text(String(localized: "settings_ios_updates"))
                .foregroundStyle(.secondary)
        }
    }
}

struct DesktopArchivedChatsPage: View {
    var projectStore: ProjectStore?
    @State private var errorMessage: String?
    private struct Row: Identifiable {
        let projectID: String?
        let session: ProjectSessionSummary
        var id: String { "\(projectID ?? "global")/\(session.sessionId)" }
    }
    private var rows: [Row] {
        guard let projectStore else { return [] }
        return projectStore.projects.flatMap { project in
            project.sessions.filter(\.isArchived).map { Row(projectID: project.record.id, session: $0) }
        } + projectStore.globalSessions.filter(\.isArchived).map { Row(projectID: nil, session: $0) }
    }
    private var activeRows: [Row] {
        guard let projectStore else { return [] }
        return projectStore.projects.flatMap { project in
            project.sessions.filter { !$0.isArchived }.map { Row(projectID: project.record.id, session: $0) }
        } + projectStore.globalSessions.filter { !$0.isArchived }.map { Row(projectID: nil, session: $0) }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let errorMessage { Text(errorMessage).foregroundStyle(.red) }
            if rows.isEmpty { ContentUnavailableView("settings_parity_empty_archive", systemImage: "archivebox") }
            if !activeRows.isEmpty {
                DisclosureGroup("settings_parity_archive_conversation") {
                    ForEach(activeRows) { row in
                        HStack {
                            Text(row.session.title)
                            Spacer()
                            Button("settings_parity_archive") {
                                Task {
                                    do { try await projectStore?.setSessionArchived(projectId: row.projectID, sessionId: row.session.sessionId, archived: true) }
                                    catch { errorMessage = error.localizedDescription }
                                }
                            }
                        }.padding(.vertical, 6)
                    }
                }
            }
            ForEach(rows) { row in
                HStack {
                    Text(row.session.title)
                    Spacer()
                    Button("settings_parity_restore") {
                        Task {
                            do { try await projectStore?.setSessionArchived(projectId: row.projectID, sessionId: row.session.sessionId, archived: false) }
                            catch { errorMessage = error.localizedDescription }
                        }
                    }.disabled(projectStore == nil)
                }
            }
        }
    }
}

struct DesktopProjectsPage: View {
    var projectStore: ProjectStore?
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let projectStore {
                ForEach(projectStore.projects) { project in
                    LabeledContent(project.record.name, value: project.record.id == projectStore.activeProjectId ? String(localized: "settings_cu_state_active") : "")
                }
            }
            LayeredSettingsPage(keys: ["trustedDirectories"])
        }
    }
}

/// 一个可切换到的项目。刻意只带 `id` 与展示用的名字：把「当前是哪个」判在
/// `ProjectStore.activeProjectId` 上，而不是拿引擎回传的路径去和客户端的
/// workspace 路径做字符串比较 —— host/guest 两套路径推导在这个仓库里已经分叉过
/// 一次，用 id 判等可以完全绕开它。
struct SettingsProjectChoice: Identifiable, Hashable {
    let id: String
    let name: String
}

/// 设置页内切换项目所需的一切，由根视图注入。
///
/// 走 Environment 而不是构造参数：`SettingsLayerPicker` 出现在四个页面上，每个页面
/// 各有自己的 `@State layer`，用构造参数就要在四处各加一个形参、再让它们各自的
/// 构造点把值传进来。而真正能执行切换的 `switchScope` 住在 `RootView` 里，离这里
/// 隔着一整层 `fullScreenCover`。
struct SettingsProjectSwitching {
    var projects: [SettingsProjectChoice] = []
    var activeProjectID: String?
    /// `nil` 表示这个宿主没法安全地换项目（例如根视图没注入），此时不画切换入口，
    /// 而不是画一个点了没反应的按钮。
    var switchTo: ((String) -> Void)?
}

private struct SettingsProjectSwitchingKey: EnvironmentKey {
    static let defaultValue = SettingsProjectSwitching()
}

extension EnvironmentValues {
    var settingsProjectSwitching: SettingsProjectSwitching {
        get { self[SettingsProjectSwitchingKey.self] }
        set { self[SettingsProjectSwitchingKey.self] = newValue }
    }
}

struct SettingsLayerPicker: View {
    @Binding var selection: DesktopSettingsLayer
    @State private var repository = DesktopSettingsRepository.shared
    @State private var showProjectList = false
    @Environment(\.settingsProjectSwitching) private var switching
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Picker(String(localized: "settings_layer_label"), selection: $selection) {
                ForEach(DesktopSettingsLayer.allCases) { Text($0.title).tag($0) }
            }.pickerStyle(.segmented)
            // 每一层都要有一句「值落到哪、影响谁」。四个裸标签本身不解释任何东西，
            // 而「项目」层的文件是随仓库提交给整个团队的。
            Text(selection.summary).font(.caption).foregroundStyle(.secondary)
            if selection.isProjectScoped { projectSection }
            if let issue = repository.layerIssue(selection) { Text(issue).font(.caption).foregroundStyle(.secondary) }
        }
    }

    @ViewBuilder private var projectSection: some View {
        if let directory = repository.projectDirectory {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("settings_parity_current_project").font(.caption2).foregroundStyle(.secondary)
                    Text(DesktopSettingsRepository.projectDisplayName(directory)).font(.callout.weight(.semibold))
                    // 名字会重复（两个项目都叫 `app`），所以完整路径永远跟着一起显示。
                    Text(directory).font(.system(.caption2, design: .monospaced)).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                if switching.switchTo != nil {
                    Button("settings_parity_switch_project") { showProjectList.toggle() }
                        .font(.caption)
                        .buttonStyle(.bordered)
                }
            }
        } else {
            // 快照还没到就不猜项目名：宁可说「还不知道」，也不要指着 B 写 A。
            Text("settings_parity_project_unknown").font(.caption).foregroundStyle(.secondary)
        }
        if showProjectList, let switchTo = switching.switchTo {
            VStack(alignment: .leading, spacing: 4) {
                // 这个后果必须先说：引擎是按会话起的，换设置的项目就等于换掉当前对话。
                Text("settings_parity_switch_project_warning").font(.caption2).foregroundStyle(.orange)
                ForEach(switching.projects) { project in
                    Button {
                        showProjectList = false
                        switchTo(project.id)
                    } label: {
                        HStack {
                            Text(project.name)
                            if project.id == switching.activeProjectID {
                                Text("settings_parity_current_project").font(.caption2).foregroundStyle(.secondary)
                            }
                            Spacer()
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

struct DesktopToolsAgentPage: View {
    @State private var repository = DesktopSettingsRepository.shared
    @State private var layer = DesktopSettingsLayer.user
    @State private var tools = ""
    @State private var outputStyle = ""
    @State private var overrideFrom = ""
    @State private var overrideTo = ""
    private let flags = ["promptSuggestionEnabled", "disableArtifact", "disableAgentView", "disableAllHooks", "skipWebFetchPreflight", "alwaysThinkingEnabled", "showThinkingSummaries", "visionDelegationEnabled"]
    private var overrides: [String: String] { repository.ownValue(key: "modelOverrides", layer: layer) as? [String: String] ?? [:] }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsConnectionStatus()
            SettingsLayerPicker(selection: $layer)
            ForEach(flags, id: \.self) { key in
                VStack(alignment: .leading) {
                    Toggle(toolFlagLabel(key), isOn: Binding(get: { repository.ownValue(key: key, layer: layer) as? Bool ?? (key == "promptSuggestionEnabled") },
                                              set: { value in save(key, value) }))
                        .disabled(!repository.canEdit(key: key, layer: layer))
                    Text(repository.provenanceLabel(for: key)).font(.caption).foregroundStyle(.secondary)
                }
            }
            GroupBox(String(localized: "settings_tools_enabled")) {
                VStack(alignment: .leading) {
                    TextField("Bash, Read, Edit", text: $tools).textFieldStyle(.roundedBorder)
                    Text(String(localized: "settings_tools_merge_hint")).font(.caption)
                    Button(String(localized: "settings_tools_save")) { save("enabledTools", tools.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }) }
                }.disabled(!repository.canEdit(key: "enabledTools", layer: layer))
            }
            GroupBox("settings_parity_output_style") {
                HStack {
                    TextField("Explanatory", text: $outputStyle).textFieldStyle(.roundedBorder)
                    Button("settings_save") { save("outputStyle", outputStyle.isEmpty ? NSNull() : outputStyle as Any) }
                }.disabled(!repository.canEdit(key: "outputStyle", layer: layer))
            }
            GroupBox(String(localized: "settings_model_overrides")) {
                VStack(alignment: .leading) {
                    ForEach(overrides.keys.sorted(), id: \.self) { key in
                        HStack {
                            Text("\(key) → \(overrides[key] ?? "")")
                            Spacer()
                            Button("settings_parity_remove", role: .destructive) { var next = overrides; next.removeValue(forKey: key); save("modelOverrides", next) }
                        }
                    }
                    TextField(String(localized: "settings_model_original_id"), text: $overrideFrom).textFieldStyle(.roundedBorder)
                    TextField(String(localized: "settings_model_replacement_id"), text: $overrideTo).textFieldStyle(.roundedBorder)
                    Button(String(localized: "settings_model_add_override")) { var next = overrides; next[overrideFrom] = overrideTo; save("modelOverrides", next) }
                        .disabled(overrideFrom.isEmpty || overrideTo.isEmpty)
                }.disabled(!repository.canEdit(key: "modelOverrides", layer: layer))
            }
        }.onAppear { reload() }
        .onChange(of: layer) { _, _ in reload() }
        .onChange(of: repository.sourceGeneration) { _, _ in reload() }
    }
    private func toolFlagLabel(_ key: String) -> String {
        switch key {
        case "promptSuggestionEnabled": String(localized: "settings_tools_prompt_suggestions")
        case "disableArtifact": String(localized: "settings_tools_disable_artifacts")
        case "disableAgentView": String(localized: "settings_tools_disable_agent_view")
        case "disableAllHooks": String(localized: "settings_tools_disable_hooks")
        case "skipWebFetchPreflight": String(localized: "settings_tools_skip_fetch_preflight")
        case "alwaysThinkingEnabled": String(localized: "settings_tools_always_thinking")
        case "showThinkingSummaries": String(localized: "settings_tools_thinking_summaries")
        case "visionDelegationEnabled": String(localized: "settings_provider_vision_delegation")
        default: key
        }
    }
    private func save(_ key: String, _ value: Any) { Task { await repository.save(key: key, json: DesktopSettingsRepository.json(value), layer: layer) } }
    private func reload() {
        tools = (repository.ownValue(key: "enabledTools", layer: layer) as? [String] ?? []).joined(separator: ", ")
        outputStyle = repository.ownValue(key: "outputStyle", layer: layer) as? String ?? ""
        overrideFrom = ""; overrideTo = ""
    }
}

struct DesktopPermissionsPage: View {
    let host: SettingsHost
    @State private var repository = DesktopSettingsRepository.shared
    @State private var layer = DesktopSettingsLayer.user
    @State private var rule = ""
    @State private var behavior = "allow"
    @State private var directory = ""
    private var own: [String: Any] { repository.ownValue(key: "permissions", layer: layer) as? [String: Any] ?? [:] }
    private var writable: Bool { repository.canEdit(key: "permissions", layer: layer) }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            SettingsLayerPicker(selection: $layer)
            Text(repository.provenanceLabel(for: "permissions")).font(.caption).foregroundStyle(.secondary)
            ForEach(["allow", "deny", "ask"], id: \.self) { kind in
                GroupBox(ruleBehaviorLabel(kind)) {
                    VStack(alignment: .leading) {
                        let rules = own[kind] as? [String] ?? []
                        if rules.isEmpty { Text(String(localized: "settings_permission_rules_empty")).foregroundStyle(.secondary) }
                        ForEach(rules, id: \.self) { value in
                            HStack {
                                Text(value).font(.system(.body, design: .monospaced)); Spacer()
                                Button("settings_parity_remove", role: .destructive) { update(kind, rules.filter { $0 != value }) }.disabled(!writable)
                            }
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            Picker(String(localized: "settings_permission_rule_behavior"), selection: $behavior) { Text(String(localized: "permission_allow")).tag("allow"); Text(String(localized: "permission_deny")).tag("deny"); Text(String(localized: "settings_permission_ask")).tag("ask") }.pickerStyle(.segmented)
            TextField(String(localized: "settings_permission_rule_placeholder"), text: $rule).textFieldStyle(.roundedBorder)
            Button(String(localized: "settings_permission_add_rule")) { let rules = own[behavior] as? [String] ?? []; if !rules.contains(rule) { update(behavior, rules + [rule]) } }
                .disabled(!writable || rule.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Picker("settings_parity_default_permission", selection: Binding(get: { own["defaultMode"] as? String ?? "default" }, set: { update("defaultMode", $0) })) {
                ForEach(["default", "acceptEdits", "plan", "dontAsk", "bypassPermissions"], id: \.self) { Text(permissionModeLabel($0)).tag($0) }
            }.disabled(!writable)
            GroupBox(String(localized: "settings_workspace_additional_directories")) {
                VStack(alignment: .leading) {
                    let directories = own["additionalDirectories"] as? [String] ?? []
                    ForEach(directories, id: \.self) { value in
                        HStack { Text(value); Spacer(); Button("settings_parity_remove", role: .destructive) { update("additionalDirectories", directories.filter { $0 != value }) } }
                    }
                    TextField(String(localized: "settings_absolute_path"), text: $directory).textFieldStyle(.roundedBorder)
                    Button(String(localized: "workspace_add_directory")) { if !directories.contains(directory) { update("additionalDirectories", directories + [directory]) } }.disabled(!directory.hasPrefix("/"))
                }.disabled(!writable)
            }
            Button(String(localized: "settings_permission_current_session")) { host.push(.permissionMode) }
        }.onChange(of: repository.sourceGeneration) { _, _ in rule = ""; directory = "" }
    }
    private func ruleBehaviorLabel(_ behavior: String) -> String {
        switch behavior {
        case "deny": String(localized: "permission_deny")
        case "ask": String(localized: "settings_permission_ask")
        default: String(localized: "permission_allow")
        }
    }
    private func permissionModeLabel(_ mode: String) -> String {
        switch mode {
        case "acceptEdits": String(localized: "settings_permission_mode_accept_edits")
        case "plan": String(localized: "settings_permission_mode_plan")
        case "dontAsk": String(localized: "settings_permission_mode_dont_ask")
        case "bypassPermissions": String(localized: "settings_permission_mode_bypass")
        default: String(localized: "settings_permission_mode_default")
        }
    }
    private func update(_ field: String, _ value: Any) {
        guard let destination = layer.destination else { return }
        var next = own; next[field] = value
        let command: ClientCommand
        if field == "defaultMode", let mode = value as? String {
            command = .setDefaultPermissionMode(destination: destination, mode: mode)
        } else {
            let old = own[field] as? [String] ?? []
            let new = value as? [String] ?? []
            let add = new.filter { !old.contains($0) }
            let remove = old.filter { !new.contains($0) }
            if field == "additionalDirectories" {
                command = .updateWorkspaceDirectories(destination: destination, add: add, remove: remove)
            } else {
                let behavior: PermissionBehaviorDto = field == "deny" ? .deny : field == "ask" ? .ask : .allow
                command = .updatePermissionRules(destination: destination, behavior: behavior, add: add, remove: remove)
            }
        }
        Task { await repository.save(key: "permissions", json: DesktopSettingsRepository.json(next), layer: layer, command: command) }
    }
}

/// One editable row of a provider's `connections` list.
///
/// A provider reachable several ways — a domestic and an international host, or
/// several API keys — holds one of these per way. Anything left blank inherits
/// the provider-level value, which is why every field here is optional except
/// the id.
private struct ProviderConnectionRow: Identifiable {
    let id = UUID()
    var connectionID = ""
    var baseURL = ""
    var type = ""
    var apiKeyEnv = ""
    /// Everything this editor does NOT surface — `models`, `credentialIds`,
    /// `region`, `apiVersion`, `pricing`, `capabilities` — kept verbatim.
    ///
    /// Saving replaces the whole `connections` array, so without this a user who
    /// merely opened the page and tapped Save would silently delete every one of
    /// those fields. The provider row above takes the same care with its models.
    private var passthrough: [String: Any] = [:]

    /// The settings dictionary this row writes: the untouched keys, with the
    /// edited ones applied and blank ones REMOVED so they inherit again.
    var settingsValue: [String: Any] {
        var out = passthrough
        out["id"] = connectionID.trimmingCharacters(in: .whitespacesAndNewlines)
        for (key, value) in [("baseUrl", baseURL), ("type", type), ("apiKeyEnv", apiKeyEnv)] {
            let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty { out.removeValue(forKey: key) } else { out[key] = trimmed }
        }
        return out
    }

    init() {}

    init(_ value: [String: Any]) {
        connectionID = value["id"] as? String ?? ""
        baseURL = value["baseUrl"] as? String ?? ""
        type = value["type"] as? String ?? ""
        apiKeyEnv = value["apiKeyEnv"] as? String ?? ""
        passthrough = value
        for key in ["id", "baseUrl", "type", "apiKeyEnv"] { passthrough.removeValue(forKey: key) }
    }
}

extension ProviderConnectionRow: Equatable {
    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.id == rhs.id
            && lhs.connectionID == rhs.connectionID
            && lhs.baseURL == rhs.baseURL
            && lhs.type == rhs.type
            && lhs.apiKeyEnv == rhs.apiKeyEnv
    }
}

struct DesktopCustomProvidersPage: View {
    var initialProfileID: String? = nil
    var initialScope: String? = nil
    @State private var initialSelectionResolved = false
    @State private var initialSelectionError: String?
    @State private var repository = DesktopSettingsRepository.shared
    @State private var layer = DesktopSettingsLayer.user
    @State private var showingImport = false
    @State private var selected: String?
    @State private var profileID = ""
    @State private var providerType = "openai"
    @State private var baseURL = ""
    @State private var apiKeyEnv = ""
    @State private var apiVersion = ""
    @State private var modelIDs = ""
    @State private var connections: [ProviderConnectionRow] = []
    @State private var alias = ""
    @State private var aliasTarget = ""
    @State private var fallbackFrom = ""
    @State private var fallbackTo = ""
    @State private var maxAttempts = 3
    @State private var backoffMS = 1000
    private let types = ["openai", "openai-responses", "anthropic", "gemini", "azure-openai", "bedrock-claude", "vertex-claude", "vertex-gemini", "foundry-claude"]
    private var own: [String: [String: Any]] { repository.ownValue(key: "providers", layer: layer) as? [String: [String: Any]] ?? [:] }
    private var routing: [String: Any] { repository.ownValue(key: "routing", layer: layer) as? [String: Any] ?? [:] }
    private var aliases: [String: String] { routing["aliases"] as? [String: String] ?? [:] }
    private var fallbacks: [String: [String]] { routing["fallback"] as? [String: [String]] ?? [:] }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsConnectionStatus()
            if let initialSelectionError {
                ContentUnavailableView(String(localized: "settings_provider_unavailable"), systemImage: "link", description: Text(initialSelectionError))
            }
            SettingsLayerPicker(selection: $layer)
            GroupBox("settings_provider_region") {
                VStack(alignment: .leading, spacing: 8) {
                    Picker("settings_provider_region", selection: Binding(
                        get: { repository.ownValue(key: "providerRegion", layer: layer) as? String ?? "" },
                        set: { save("providerRegion", $0.isEmpty ? NSNull() : $0 as Any) }
                    )) {
                        Text("settings_parity_inherit").tag("")
                        Text("settings_provider_region_international").tag("international")
                        Text("settings_provider_region_china").tag("china_mainland")
                    }
                    .disabled(!repository.canEdit(key: "providerRegion", layer: layer))
                    .accessibilityIdentifier("settings.providers.region")
                    Text("settings_provider_region_help").font(.caption).foregroundStyle(.secondary)
                    Text(repository.provenanceLabel(for: "providerRegion")).font(.caption).foregroundStyle(.secondary)
                }
            }

            Text(repository.provenanceLabel(for: "providers")).font(.caption).foregroundStyle(.secondary)
            ForEach(own.keys.sorted(), id: \.self) { id in
                HStack {
                    Button(id) { select(id) }; Spacer()
                    Text(own[id]?["type"] as? String ?? "").foregroundStyle(.secondary)
                    Button("settings_parity_remove", role: .destructive) { var next = own; next.removeValue(forKey: id); save("providers", next) }
                        .disabled(!repository.canEdit(key: "providers", layer: layer))
                }
            }
            HStack {
                Button("settings_parity_add_profile") { select(nil) }
                Button("settings_parity_import_providers") { showingImport = true }
                    .disabled(!repository.canEdit(key: "providers", layer: layer))
            }
            GroupBox(String(localized: "settings_provider_profile")) {
                VStack(alignment: .leading, spacing: 10) {
                    TextField("settings_parity_new_profile", text: $profileID).disabled(selected != nil)
                    Picker(String(localized: "settings_provider_protocol"), selection: $providerType) { ForEach(types, id: \.self) { Text($0).tag($0) } }
                    TextField("settings_parity_base_url", text: $baseURL).keyboardType(.URL)
                    TextField("settings_parity_api_key_env", text: $apiKeyEnv)
                    Text(String(localized: "settings_provider_credentials_separate")).font(.caption).foregroundStyle(.secondary)
                    if providerType == "azure-openai" { TextField(String(localized: "settings_provider_api_version"), text: $apiVersion) }
                    TextField(String(localized: "settings_provider_model_ids"), text: $modelIDs)
                    Button("settings_save") { saveProvider() }.accessibilityIdentifier("settings.providers.save")
                }.textFieldStyle(.roundedBorder).autocorrectionDisabled().textInputAutocapitalization(.never)
                    .disabled(!repository.canEdit(key: "providers", layer: layer))
            }
            GroupBox(String(localized: "settings_provider_connections")) {
                VStack(alignment: .leading, spacing: 10) {
                    Text(String(localized: "settings_provider_connections_layered_hint"))
                        .font(.caption).foregroundStyle(.secondary)
                    if connections.isEmpty {
                        Text(String(localized: "settings_provider_connections_single"))
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach($connections) { $connection in
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                TextField(String(localized: "settings_provider_connection_id"), text: $connection.connectionID)
                                    .accessibilityIdentifier("settings.providers.connection.id")
                                Button("settings_parity_remove", role: .destructive) {
                                    connections.removeAll { $0.id == connection.id }
                                }
                            }
                            TextField("settings_parity_base_url", text: $connection.baseURL).keyboardType(.URL)
                            Picker(String(localized: "settings_provider_protocol"), selection: $connection.type) {
                                Text(String(localized: "settings_provider_inherit_protocol \(providerType)")).tag("")
                                ForEach(types, id: \.self) { Text($0).tag($0) }
                            }
                            TextField("settings_parity_api_key_env", text: $connection.apiKeyEnv)
                            Text(String(localized: "settings_provider_connection_credential_hint \(profileID.isEmpty ? String(localized: "model_detail_provider") : profileID)"))
                                .font(.caption2).foregroundStyle(.secondary)
                        }
                        .padding(.vertical, 4)
                    }
                    Button(String(localized: "settings_provider_connection_add")) { connections.append(ProviderConnectionRow()) }
                        .accessibilityIdentifier("settings.providers.connection.add")
                }.textFieldStyle(.roundedBorder).autocorrectionDisabled().textInputAutocapitalization(.never)
                    .disabled(!repository.canEdit(key: "providers", layer: layer))
            }
            EngineProfileCredentialsEditor(initialProviderID: profileID)
            GroupBox(String(localized: "settings_provider_routing_aliases")) {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(aliases.keys.sorted(), id: \.self) { name in
                        HStack { Text("\(name) → \(aliases[name] ?? "")"); Spacer(); Button("settings_parity_remove", role: .destructive) { var next = aliases; next.removeValue(forKey: name); saveRouting("aliases", next) } }
                    }
                    TextField("settings_parity_alias", text: $alias)
                    TextField(String(localized: "settings_provider_route_target"), text: $aliasTarget)
                    Button("settings_parity_add_alias") { var next = aliases; next[alias] = aliasTarget; saveRouting("aliases", next) }.disabled(alias.isEmpty || aliasTarget.isEmpty)
                }.textFieldStyle(.roundedBorder).disabled(!repository.canEdit(key: "routing", layer: layer))
            }
            GroupBox(String(localized: "settings_provider_fallback_routes")) {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(fallbacks.keys.sorted(), id: \.self) { name in
                        HStack { Text("\(name) → \((fallbacks[name] ?? []).joined(separator: ", "))"); Spacer(); Button("settings_parity_remove", role: .destructive) { var next = fallbacks; next.removeValue(forKey: name); saveRouting("fallback", next) } }
                    }
                    TextField(String(localized: "settings_provider_primary_route"), text: $fallbackFrom)
                    TextField(String(localized: "settings_provider_fallback_routes_placeholder"), text: $fallbackTo)
                    Button(String(localized: "settings_provider_add_fallback")) { var next = fallbacks; next[fallbackFrom] = split(fallbackTo); saveRouting("fallback", next) }.disabled(fallbackFrom.isEmpty || fallbackTo.isEmpty)
                    Stepper(String(localized: "settings_provider_max_attempts \(maxAttempts)"), value: $maxAttempts, in: 1...20)
                    Stepper(String(localized: "settings_provider_backoff \(backoffMS)"), value: $backoffMS, in: 0...60000, step: 100)
                    Button(String(localized: "settings_provider_save_retry")) {
                        var retry = routing["retry"] as? [String: Any] ?? [:]
                        retry["maxAttempts"] = maxAttempts; retry["backoffMs"] = backoffMS
                        saveRouting("retry", retry)
                    }
                }.textFieldStyle(.roundedBorder).disabled(!repository.canEdit(key: "routing", layer: layer))
            }
            DisclosureGroup(String(localized: "settings_provider_routing_json")) { SettingsValueEditor(key: "providers", layer: layer); SettingsValueEditor(key: "routing", layer: layer) }
        }.disabled(initialSelectionError != nil)
        .onAppear { if initialProfileID == nil { select(nil) } else { resolveLinkedProfile() } }
        .sheet(isPresented: $showingImport) { ProviderBulkImportPage(layer: layer) }
        .onChange(of: layer) { _, _ in
            if let initialProfileID, selected == initialProfileID, own[initialProfileID] != nil { select(initialProfileID) }
            else { reset() }
        }
        .onChange(of: repository.revision) { _, _ in resolveLinkedProfile() }
        .onChange(of: repository.sourceGeneration) { _, _ in
            initialSelectionResolved = false; initialSelectionError = nil; reset()
        }
    }
    private func resolveLinkedProfile() {
        guard let initialProfileID, !initialSelectionResolved, repository.loaded else { return }
        initialSelectionResolved = true
        let matches = DesktopSettingsLayer.allCases.filter { candidate in
            (initialScope == nil || initialScope == candidate.rawValue) && (repository.ownValue(key: "providers", layer: candidate) as? [String: Any])?[initialProfileID] != nil
        }
        guard matches.count == 1, let match = matches.first else {
            initialSelectionError = matches.count > 1 ? String(localized: "provider_link_ambiguous") : String(localized: "provider_link_missing")
            return
        }
        layer = match; select(initialProfileID)
    }
    private func select(_ id: String?) {
        selected = id; profileID = id ?? ""
        let value = id.flatMap { own[$0] } ?? [:]
        providerType = value["type"] as? String ?? "openai"
        baseURL = value["baseUrl"] as? String ?? ""
        apiKeyEnv = value["apiKeyEnv"] as? String ?? ""
        apiVersion = value["apiVersion"] as? String ?? ""
        modelIDs = (value["models"] as? [[String: Any]] ?? []).compactMap { $0["id"] as? String }.joined(separator: ", ")
        connections = (value["connections"] as? [[String: Any]] ?? []).map(ProviderConnectionRow.init)
    }
    private func reset() { select(nil); alias = ""; aliasTarget = ""; fallbackFrom = ""; fallbackTo = "" }  // select() reloads `connections`
    private func split(_ text: String) -> [String] { text.split(whereSeparator: { $0 == "," || $0 == "\n" }).map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty } }
    private func saveProvider() {
        let id = profileID.trimmingCharacters(in: .whitespacesAndNewlines)
        let models = split(modelIDs)
        guard id.range(of: "^[a-z0-9][a-z0-9._-]{0,63}$", options: .regularExpression) != nil,
              !["builtin", "claude", "__proto__", "prototype", "constructor"].contains(id),
              !models.isEmpty, Set(models).count == models.count else { repository.errorMessage = String(localized: "settings_parity_provider_validation_invalid"); return }
        if selected == nil && own[id] != nil { repository.errorMessage = String(localized: "settings_parity_provider_validation_invalid"); return }
        if !baseURL.isEmpty {
            guard let url = URL(string: baseURL), ["http", "https"].contains(url.scheme ?? ""), url.host != nil else { repository.errorMessage = String(localized: "settings_parity_provider_validation_url"); return }
        }
        if !apiKeyEnv.isEmpty && apiKeyEnv.range(of: "^[A-Za-z_][A-Za-z0-9_]*$", options: .regularExpression) == nil { repository.errorMessage = String(localized: "settings_parity_provider_validation_env"); return }
        if providerType == "azure-openai" && apiVersion.isEmpty { repository.errorMessage = String(localized: "settings_parity_provider_validation_invalid"); return }
        if let error = validateConnections() { repository.errorMessage = error; return }
        var provider = own[id] ?? [:]
        let oldModels = provider["models"] as? [[String: Any]] ?? []
        provider["type"] = providerType
        provider["baseUrl"] = baseURL.isEmpty ? nil : baseURL
        provider["apiKeyEnv"] = apiKeyEnv.isEmpty ? nil : apiKeyEnv
        if !apiVersion.isEmpty { provider["apiVersion"] = apiVersion }
        provider["models"] = models.map { id in oldModels.first { $0["id"] as? String == id } ?? ["id": id] }
        // An empty list must REMOVE the key rather than persist `connections: []`,
        // which the engine rejects as "declare connections or omit the field".
        provider["connections"] = connections.isEmpty ? nil : connections.map(\.settingsValue)
        var next = own; next[id] = provider
        save("providers", next)
    }
    /// Reject connection rows the engine would refuse, naming the reason.
    ///
    /// Mirrors the desktop editor and the engine's own desugaring: an id becomes
    /// part of a qualified model reference (`provider:connection/model`), so a
    /// separator inside one produces a reference that cannot be routed, and two
    /// rows sharing an id would desugar to two profiles with the same name.
    private func validateConnections() -> String? {
        guard !connections.isEmpty else { return nil }
        var seen: Set<String> = []
        for connection in connections {
            let id = connection.connectionID.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty, !id.contains("/"), !id.contains(":"), !id.contains("#") else {
                return String(localized: "settings_parity_provider_validation_invalid")
            }
            guard seen.insert(id).inserted else {
                return String(localized: "settings_parity_provider_validation_invalid")
            }
            // Each connection is validated as the flat provider it desugars to:
            // its own address when it sets one, otherwise the inherited address,
            // and at least one of the two must be usable.
            let effective = connection.baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? baseURL : connection.baseURL
            let trimmed = effective.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty {
                return String(localized: "settings_parity_provider_validation_url")
            }
            guard let url = URL(string: trimmed), ["http", "https"].contains(url.scheme ?? ""), url.host != nil else {
                return String(localized: "settings_parity_provider_validation_url")
            }
            let env = connection.apiKeyEnv.trimmingCharacters(in: .whitespacesAndNewlines)
            if !env.isEmpty, env.range(of: "^[A-Za-z_][A-Za-z0-9_]*$", options: .regularExpression) == nil {
                return String(localized: "settings_parity_provider_validation_env")
            }
        }
        return nil
    }

    private func saveRouting(_ key: String, _ value: Any) { var next = routing; next[key] = value; save("routing", next) }
    private func save(_ key: String, _ value: Any) { Task { await repository.save(key: key, json: DesktopSettingsRepository.json(value), layer: layer) } }
}

struct EngineProfileCredentialsEditor: View {
    var initialProviderID = ""
    @State private var repository = DesktopSettingsRepository.shared
    @State private var providerID = ""
    @State private var secret = ""
    private var ids: [String] { repository.credentialStates.keys.sorted() }
    var body: some View {
        GroupBox("settings_parity_credentials") {
            VStack(alignment: .leading, spacing: 12) {
                Text(String(localized: "settings_credential_storage_separate"))
                    .font(.caption).foregroundStyle(.secondary)
                if let encrypted = repository.credentialStorageEncrypted {
                    Label(encrypted ? String(localized: "settings_credential_storage_encrypted") : String(localized: "settings_credential_storage_unencrypted"), systemImage: encrypted ? "lock.fill" : "lock.open")
                        .font(.caption)
                }
                if !ids.isEmpty {
                    Picker(String(localized: "model_detail_provider"), selection: $providerID) {
                        Text(String(localized: "settings_provider_choose")).tag("")
                        ForEach(ids, id: \.self) { id in Text(id).tag(id) }
                    }
                }
                TextField(String(localized: "settings_credential_profile_id"), text: $providerID).textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled().textInputAutocapitalization(.never)
                if let configured = repository.credentialStates[providerID] {
                    Text(configured ? String(localized: "settings_parity_secret_configured") : String(localized: "settings_parity_secret_missing")).font(.caption)
                }
                SecureField(String(localized: "settings_provider_api_key"), text: $secret).textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled().textInputAutocapitalization(.never)
                HStack {
                    Button("settings_save") {
                        let id = providerID
                        let value = secret
                        secret = ""
                        Task { await repository.saveCredential(providerID: id, secret: value) }
                    }.disabled(!repository.connected || repository.saving || providerID.isEmpty || secret.isEmpty)
                    Button(String(localized: "settings_provider_clear_credential"), role: .destructive) {
                        let id = providerID
                        secret = ""
                        Task { await repository.saveCredential(providerID: id, secret: nil) }
                    }.disabled(!repository.connected || repository.saving || repository.credentialStates[providerID] != true)
                    Button("settings_linux_refresh_button") { Task { await repository.refreshCredentials() } }.disabled(!repository.connected)
                }
            }
        }.onAppear { providerID = initialProviderID }
        .onChange(of: initialProviderID) { _, value in providerID = value; secret = "" }
        .onChange(of: repository.sourceGeneration) { _, _ in providerID = ""; secret = "" }
        .onChange(of: providerID) { _, _ in secret = "" }
        .task(id: repository.sourceGeneration) { await repository.refreshCredentials() }
    }
}

/// Applies durable writes explicitly. Reconnection recreates engine-owned
/// state, so warn before discarding any still-unsaved page drafts.
struct SettingsApplyChangesBanner: View {
    let reconnect: (() async throws -> Void)?
    @State private var repository = DesktopSettingsRepository.shared
    @State private var secrets = PluginSecretRepository.shared
    @State private var confirming = false
    @State private var applying = false
    @State private var errorMessage: String?
    @State private var applied = false

    private var busy: Bool { repository.saving || secrets.busy || applying }
    private var pending: Bool { repository.needsReconnect || secrets.needsReconnect }

    var body: some View {
        if repository.connected {
            VStack(alignment: .leading, spacing: 8) {
                if pending {
                    Label("settings_parity_saved_pending", systemImage: "arrow.triangle.2.circlepath")
                        .font(.footnote.weight(.medium))
                        .accessibilityIdentifier("settings.pending-changes")
                } else if applied {
                    Text("settings_parity_applied_connection").font(.footnote).foregroundStyle(.secondary)
                }
                if reconnect != nil {
                    Button("settings_parity_apply_saved") { confirming = true }
                        .disabled(busy || !repository.loaded)
                        .accessibilityIdentifier("settings.apply-saved")
                    Text("settings_parity_save_drafts_first").font(.caption).foregroundStyle(.secondary)
                }
                if applying { ProgressView("settings_parity_applying") }
                if let errorMessage { Text(errorMessage).font(.caption).foregroundStyle(.red) }
            }
            .onChange(of: repository.sourceGeneration) { _, _ in confirming = false; applied = false; errorMessage = nil }
            .confirmationDialog("settings_parity_apply_confirm", isPresented: $confirming, titleVisibility: .visible) {
                Button("settings_parity_apply_saved", role: .destructive) { apply() }
                Button("common_cancel", role: .cancel) {}
            } message: {
                Text("settings_parity_save_drafts_first")
            }
        }
    }

    private func apply() {
        guard let reconnect, !busy else { return }
        applying = true
        errorMessage = nil
        applied = false
        Task { @MainActor in
            do {
                // This callback is also used for secret-cache invalidation; it
                // rejects active turns/tasks and never cancels ongoing work.
                if secrets.needsReconnect {
                    await secrets.reconnect(using: reconnect)
                    if let error = secrets.errorMessage { throw ProviderBulkImport.ImportError(error) }
                } else { try await reconnect() }
                applied = true
            } catch { errorMessage = error.localizedDescription }
            applying = false
        }
    }
}
