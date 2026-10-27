import SwiftUI

struct DesktopHooksPage: View {
    @State private var repository = DesktopSettingsRepository.shared
    @State private var layer = DesktopSettingsLayer.user
    @State private var draft = "{}"
    private var document: [String: Any] { DesktopSettingsRepository.object(repository.documents["hook"] ?? "{}") ?? [:] }
    private var revision: String? { document["revision_sha256"] as? String }
    private var writable: Bool {
        repository.canEdit(key: "hooks", layer: layer) && document["scope"] as? String == layer.rawValue && revision != nil
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            Picker(String(localized: "settings_layer_label"), selection: $layer) {
                ForEach(DesktopSettingsLayer.allCases) { Text($0.title).tag($0) }
            }.pickerStyle(.segmented)
            Text(String(localized: "settings_hooks_intro")).font(.footnote)
            TextEditor(text: $draft).font(.system(.body, design: .monospaced)).frame(minHeight: 260)
                .autocorrectionDisabled().textInputAutocapitalization(.never).disabled(!writable)
            HStack {
                Button(String(localized: "settings_reload")) { Task { await load() } }.disabled(!repository.connected)
                Button("settings_parity_validate_hooks") { Task { await submit(action: "validate_document") } }.disabled(!writable)
                Button("settings_save") { Task { await submit(action: "save_document") } }.disabled(!writable)
            }
            DisclosureGroup("settings_parity_effective_hooks") {
                Text(document["effective_json"] as? String ?? "{}").font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
        }
        .task(id: "\(layer.rawValue)-\(repository.sourceGeneration)") { await load() }
        .onChange(of: repository.sourceGeneration) { _, _ in draft = "{}" }
        .onChange(of: repository.documents["hook"]) { _, _ in
            guard document["scope"] as? String == layer.rawValue else { return }
            draft = document["own_json"] as? String ?? "{}"
        }
    }
    private func load() async { await repository.admin(domain: "hook", action: "get_document", scope: layer.rawValue) }
    private func submit(action: String) async {
        guard let value = DesktopSettingsRepository.object(draft) else { repository.errorMessage = String(localized: "settings_hooks_json_required"); return }
        await repository.admin(domain: "hook", action: action, scope: layer.rawValue, revision: revision,
                               payload: DesktopSettingsRepository.json(["scope": layer.rawValue, "hooks": value]), mutation: true)
    }
}

struct DesktopPluginsPage: View {
    let host: SettingsHost
    @State private var repository = DesktopSettingsRepository.shared
    private var catalog: [String: Any] { DesktopSettingsRepository.object(repository.catalogs["plugin"] ?? "{}") ?? [:] }
    private var installed: [[String: Any]] { catalog["installed"] as? [[String: Any]] ?? [] }
    private var marketplaces: [[String: Any]] { catalog["marketplaces"] as? [[String: Any]] ?? [] }
    private var available: [[String: Any]] { catalog["available"] as? [[String: Any]] ?? [] }
    @State private var layer = DesktopSettingsLayer.user
    @State private var marketplaceSource = ""
    @State private var previewPayload: [String: Any]?
    @State private var previewRevision: String?
    @State private var previewSummary: String?
    private var writable: Bool { repository.canEdit(key: "enabledPlugins", layer: layer) }
    private var currentRevision: String? { (catalog["revisions"] as? [String: String])?[layer.rawValue] }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            Button(String(localized: "plugins_refresh_all")) { Task { await load() } }.disabled(!repository.connected)
            SettingsLayerPicker(selection: $layer)
            ForEach(installed.compactMap { $0["id"] as? String }, id: \.self) { id in
                let row = installed.first { $0["id"] as? String == id }
                GroupBox(row?["display_name"] as? String ?? id) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(row?["version"] as? String ?? "")
                        PluginSensitiveFields(pluginID: id, schemaJSON: row?["config_schema_json"] as? String,
                                              readOnly: !repository.canEdit(key: "pluginConfigs", layer: layer), reconnect: host.onReconnectAfterSecretChange)
                        HStack {
                            Button(String(localized: "settings_provider_enable")) { preview(action: "enable", target: ["plugin": id]) }
                            Button(String(localized: "plugins_action_disable")) { preview(action: "disable", target: ["plugin": id]) }
                            if row?["source"] as? String != "builtin" {
                                Button(String(localized: "plugins_action_update")) { preview(action: "update", target: ["plugin": id]) }
                                Button(String(localized: "plugins_action_uninstall"), role: .destructive) { preview(action: "uninstall", target: ["plugin": id]) }
                            }
                        }.disabled(!writable)
                    }
                }
            }
            ForEach(marketplaces.compactMap { $0["name"] as? String }, id: \.self) { name in
                HStack {
                    Label(name, systemImage: "shippingbox")
                    Spacer()
                    Button(String(localized: "plugins_action_update")) { preview(action: "marketplace_update", target: ["name": name]) }
                    Button("settings_parity_remove", role: .destructive) { preview(action: "marketplace_remove", target: ["name": name]) }
                }.disabled(!writable)
            }
            if repository.catalogs["plugin"] != nil && installed.isEmpty { Text(String(localized: "plugins_installed_empty")).foregroundStyle(.secondary) }
            ForEach(available.compactMap { $0["id"] as? String }, id: \.self) { id in
                HStack {
                    Text(id)
                    Spacer()
                    Button(String(localized: "plugins_install_or_update")) { preview(action: available.first { $0["id"] as? String == id }?["installed"] as? Bool == true ? "update" : "install", target: ["plugin": id]) }.disabled(!writable)
                }
            }
            TextField("settings_parity_marketplace_source", text: $marketplaceSource).textFieldStyle(.roundedBorder)
            Button("settings_parity_preview_marketplace") { preview(action: "marketplace_add", target: ["source": marketplaceSource]) }
                .disabled(!writable || marketplaceSource.isEmpty)
            if let previewSummary {
                GroupBox(String(localized: "plugins_review_operation")) {
                    VStack(alignment: .leading, spacing: 10) {
                        Text(previewSummary).textSelection(.enabled)
                        Button("settings_parity_confirm_apply") { applyPreview() }.disabled(!writable || previewPayload == nil)
                        Button("common_cancel") { self.previewSummary = nil; previewPayload = nil }
                    }
                }
            }
            DisclosureGroup(String(localized: "plugins_advanced")) { ForEach(["enabledPlugins", "pluginConfigs", "extraKnownMarketplaces"], id: \.self) { PluginConfigurationEditor(key: $0, layer: layer) } }
        }.task(id: repository.sourceGeneration) { await load() }
        .onChange(of: repository.documents["plugin"]) { _, json in
            guard let json, let doc = DesktopSettingsRepository.object(json),
                  let payload = previewPayload, doc["action"] as? String == payload["action"] as? String else { return }
            previewSummary = doc["summary"] as? String
        }
        .onChange(of: layer) { _, _ in previewPayload = nil; previewSummary = nil }
        .onChange(of: repository.sourceGeneration) { _, _ in previewPayload = nil; previewSummary = nil }
    }
    private func load() async { await repository.admin(domain: "plugin", action: "get_catalog") }
    private func preview(action: String, target: [String: String]) {
        guard let currentRevision else { repository.errorMessage = String(localized: "plugins_catalog_refresh_first"); return }
        var payload: [String: Any] = target
        payload["action"] = action; payload["scope"] = layer.rawValue
        previewPayload = payload; previewRevision = currentRevision; previewSummary = nil
        Task { await repository.admin(domain: "plugin", action: "preview_operation", scope: layer.rawValue, revision: currentRevision,
                                      payload: DesktopSettingsRepository.json(payload), mutation: true) }
    }
    private func applyPreview() {
        guard var payload = previewPayload, let previewRevision, previewSummary != nil else { return }
        payload["confirmed"] = true
        previewPayload = nil; previewSummary = nil
        Task { await repository.admin(domain: "plugin", action: "apply_operation", scope: layer.rawValue, revision: previewRevision,
                                      payload: DesktopSettingsRepository.json(payload), mutation: true) }
    }
}

/// MCP's three storage scopes are independent of settings file layers.
private enum MCPAdminDirectoryTab: String, Hashable {
    case publicDirectory
    case personal
}

private enum PendingPublicMCPAction {
    case useTemplate(PublicExtensionItem)
    case openExisting(name: String)
}

struct DesktopMCPAdminPage: View {
    let host: SettingsHost
    @Environment(\.openURL) private var openURL
    var initialName: String? = nil
    var initialScope: String? = nil
    @State private var initialSelectionError: String?
    @State private var initialSelectionResolved = false
    @State private var repository = DesktopSettingsRepository.shared
    @State private var scope = "user"
    @State private var selectedName: String?
    @State private var name = ""
    @State private var draft = "{}"
    @State private var baselineName = ""
    @State private var baselineDraft = "{}"
    @State private var draftRevision: String?
    @State private var directoryTab = MCPAdminDirectoryTab.personal
    @State private var pendingPublicMCPAction: PendingPublicMCPAction?
    @State private var confirmsTemplateReplacement = false
    @State private var directoryConfirmationTitle = ""
    @State private var directoryConfirmationMessage = ""
    private var snapshot: [String: Any] { DesktopSettingsRepository.object(repository.catalogs["mcp"] ?? "{}") ?? [:] }
    private var scopeRow: [String: Any] {
        (snapshot["scopes"] as? [[String: Any]] ?? []).first { $0["scope"] as? String == scope } ?? [:]
    }
    private var servers: [String: Any] {
        let root = DesktopSettingsRepository.object(scopeRow["raw_json"] as? String ?? "{}") ?? [:]
        return root["mcpServers"] as? [String: Any] ?? root
    }
    /// 当前作用域的落盘位置。具体路径不写死在文案里 —— 引擎在 `scopeRow["path"]`
    /// 上回传真实路径，就在下面一行显示；这里只讲含义与影响范围。
    private var mcpScopeDescription: String {
        switch scope {
        case "project": String(localized: "mcp_scope_project_desc")
        case "local": String(localized: "mcp_scope_local_desc")
        default: String(localized: "mcp_scope_user_desc")
        }
    }
    private var mcpScopeLabel: String {
        switch scope {
        case "project": String(localized: "mcp_scope_project_title")
        case "local": String(localized: "mcp_scope_local_title")
        default: String(localized: "settings_parity_user_layer")
        }
    }
    private func templateMatch(_ item: PublicExtensionItem) -> PublicMCPTemplateMatch? {
        guard let config = item.mcpConfig else { return nil }
        return publicMCPTemplateMatch(name: item.mcpName ?? item.name, template: config, configuredServers: servers)
    }
    private var writable: Bool { repository.connected && !repository.saving && draftRevision != nil && initialSelectionError == nil }
    private var hasUnsavedDraft: Bool { name != baselineName || draft != baselineDraft }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Picker(String(localized: "public_extensions_source"), selection: $directoryTab) {
                Text("public_extensions_public_tab").tag(MCPAdminDirectoryTab.publicDirectory)
                Text("public_extensions_personal_tab").tag(MCPAdminDirectoryTab.personal)
            }.pickerStyle(.segmented)

            if directoryTab == .publicDirectory {
                Text("public_extensions_mcp_note")
                    .font(.footnote).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(String(format: String(localized: "public_extensions_mcp_scope_format"), mcpScopeLabel))
                    .font(.caption).foregroundStyle(.secondary)
                PublicExtensionDirectoryList(
                    kind: .mcp,
                    actionTitle: { item in
                        switch templateMatch(item) {
                        case .some(.sameConfiguration):
                            return String(localized: "public_extensions_mcp_open_existing")
                        case .some(.nameConflict):
                            return String(localized: "public_extensions_mcp_name_conflict_action")
                        case .some(.notConfigured):
                            return String(localized: "public_extensions_use_template")
                        case .none:
                            return String(localized: "public_extensions_open_documentation")
                        }
                    },
                    action: { item in
                        switch templateMatch(item) {
                        case .some(.sameConfiguration):
                            requestOpenExisting(item)
                        case .some(.nameConflict):
                            requestTemplate(item, hasNameConflict: true)
                        case .some(.notConfigured):
                            requestTemplate(item)
                        case .none:
                            openOfficialDocumentation(for: item)
                        }
                    }
                )
            } else {
                SettingsConnectionStatus()
                if let initialSelectionError {
                    ContentUnavailableView(String(localized: "mcp_server_unavailable"), systemImage: "link", description: Text(initialSelectionError))
                }
                Picker(String(localized: "mcp_storage_scope"), selection: $scope) {
                    Text(String(localized: "settings_parity_user_layer")).tag("user"); Text(String(localized: "mcp_scope_local_title")).tag("local"); Text(String(localized: "mcp_scope_project_title")).tag("project")
                }.pickerStyle(.segmented)
                // MCP 的三个作用域和设置页的层用了同样的词，却是两套独立存储 —— 而且
                // 「本地」的含义正好相反：设置层的 local 在项目内，MCP 的 local 在主目录。
                // 刚在设置页学会一个意思的人，到这一页会把同一个词读成同一个意思。
                Text(mcpScopeDescription).font(.caption).foregroundStyle(.secondary)
                Text("mcp_scope_vs_layers_note").font(.caption2).foregroundStyle(.secondary)
                Text(scopeRow["path"] as? String ?? String(localized: "mcp_scope_load_hint"))
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                DisclosureGroup(String(localized: "mcp_runtime_servers")) {
                    ForEach(host.convo.mcpServers) { server in
                        HStack {
                            Text(server.name)
                            Spacer()
                            Text(String(describing: server.status)).foregroundStyle(.secondary)
                        }.padding(.vertical, 4)
                    }
                }
                ForEach(servers.keys.sorted(), id: \.self) { serverName in
                    Button(serverName) { select(serverName) }
                }
                HStack {
                    Button("settings_linux_refresh_button") { Task { await load() } }.disabled(!repository.connected)
                    Button("settings_parity_new_server") { select(nil) }.disabled(scopeRow.isEmpty)
                }
                TextField("settings_parity_server_name", text: $name).textFieldStyle(.roundedBorder)
                    .disabled(selectedName != nil || !writable).autocorrectionDisabled().textInputAutocapitalization(.never)
                TextEditor(text: $draft).font(.system(.body, design: .monospaced)).frame(minHeight: 180)
                    .disabled(!writable).autocorrectionDisabled().textInputAutocapitalization(.never)
                HStack {
                    Button("settings_parity_save_server") { Task { await save(remove: false) } }.disabled(!writable || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    if selectedName != nil {
                        Button("settings_parity_remove_server", role: .destructive) { Task { await save(remove: true) } }.disabled(!writable)
                    }
                }
            }
        }.task(id: repository.sourceGeneration) { await load() }
        .onChange(of: scope) { _, newScope in
            if let initialName, selectedName == initialName,
               MCPDeepLinkSelection.resolve(name: initialName, requestedScope: initialScope, snapshot: snapshot) == .selected(scope: newScope) { select(initialName) }
            else { select(nil) }
        }
        .onChange(of: repository.sourceGeneration) { _, _ in
            initialSelectionResolved = false; initialSelectionError = nil; select(nil)
        }
        .onChange(of: repository.catalogs["mcp"]) { _, _ in
            resolveInitialSelection()
            if let selectedName, servers[selectedName] != nil {
                if hasUnsavedDraft {
                    draftRevision = scopeRow["revision_sha256"] as? String
                } else {
                    select(selectedName)
                }
            } else if selectedName == nil {
                // A public template is an unsaved draft. Catalog refreshes must
                // update its revision without replacing its raw JSON contents.
                draftRevision = scopeRow["revision_sha256"] as? String
            } else if !hasUnsavedDraft {
                select(nil)
            }
        }
        .alert(directoryConfirmationTitle, isPresented: $confirmsTemplateReplacement) {
            Button(String(localized: "public_extensions_continue"), role: .destructive) {
                switch pendingPublicMCPAction {
                case .some(.useTemplate(let item)): applyTemplate(item)
                case .some(.openExisting(let name)):
                    select(name)
                    directoryTab = .personal
                case .none: break
                }
                pendingPublicMCPAction = nil
            }
            Button(String(localized: "common_cancel"), role: .cancel) { pendingPublicMCPAction = nil }
        } message: {
            Text(directoryConfirmationMessage)
        }
    }
    private func load() async {
        await repository.admin(domain: "mcp", action: "get_snapshot")
        resolveInitialSelection()
    }
    private func resolveInitialSelection() {
        guard let initialName, !initialSelectionResolved, !snapshot.isEmpty else { return }
        initialSelectionResolved = true
        switch MCPDeepLinkSelection.resolve(name: initialName, requestedScope: initialScope, snapshot: snapshot) {
        case .selected(let selectedScope): scope = selectedScope; select(initialName)
        case .missing: initialSelectionError = String(localized: "mcp_link_missing")
        case .ambiguous: initialSelectionError = String(localized: "mcp_link_ambiguous")
        case .invalidScope: initialSelectionError = String(localized: "mcp_link_scope_invalid")
        }
    }
    private func select(_ selected: String?) {
        selectedName = selected
        name = selected ?? ""
        draft = DesktopSettingsRepository.json(selected.flatMap { servers[$0] } ?? [:])
        baselineName = name
        baselineDraft = draft
        draftRevision = scopeRow["revision_sha256"] as? String
    }

    private func requestTemplate(_ item: PublicExtensionItem, hasNameConflict: Bool = false) {
        guard case .some = item.mcpConfig else { return }
        guard hasNameConflict || hasUnsavedDraft else {
            applyTemplate(item)
            return
        }
        pendingPublicMCPAction = .useTemplate(item)
        if hasNameConflict {
            directoryConfirmationTitle = String(localized: "public_extensions_mcp_name_conflict_title")
            directoryConfirmationMessage = String(localized: "public_extensions_mcp_name_conflict_message")
        } else {
            directoryConfirmationTitle = String(localized: "public_extensions_replace_title")
            directoryConfirmationMessage = String(localized: "public_extensions_replace_message")
        }
        confirmsTemplateReplacement = true
    }

    private func requestOpenExisting(_ item: PublicExtensionItem) {
        let name = item.mcpName ?? item.name
        guard hasUnsavedDraft else {
            select(name)
            directoryTab = .personal
            return
        }
        pendingPublicMCPAction = .openExisting(name: name)
        directoryConfirmationTitle = String(localized: "public_extensions_open_existing_title")
        directoryConfirmationMessage = String(localized: "public_extensions_open_existing_message")
        confirmsTemplateReplacement = true
    }

    private func applyTemplate(_ item: PublicExtensionItem) {
        guard let config = item.mcpConfig else { return }
        selectedName = nil
        name = item.mcpName ?? item.name
        draft = DesktopSettingsRepository.json(config)
        draftRevision = scopeRow["revision_sha256"] as? String
        directoryTab = .personal
    }

    private func openOfficialDocumentation(for item: PublicExtensionItem) {
        guard let url = URL(string: item.documentationURL), url.scheme?.lowercased() == "https" else { return }
        openURL(url)
    }

    private func save(remove: Bool) async {
        guard writable, let config = DesktopSettingsRepository.object(draft) else { repository.errorMessage = String(localized: "mcp_json_object_required"); return }
        let payload: [String: Any] = ["scope": scope, "name": name, "config": config]
        await repository.admin(domain: "mcp", action: remove ? "remove_server" : "save_server", target: name, scope: scope,
                               revision: draftRevision, payload: DesktopSettingsRepository.json(payload), mutation: true)
    }
}

struct DesktopSkillsAdminPage: View {
    var initialID: String? = nil
    var initialScope: String? = nil
    @State private var initialSkillError: String?
    @State private var openedInitialID = false
    @State private var repository = DesktopSettingsRepository.shared
    @State private var target: SkillAdminIdentity?
    @State private var scope = "user"
    @State private var name = ""
    @State private var markdown = ""
    @State private var revision: String?
    @State private var writable = false
    private var entries: [SkillAdminCatalogEntry] { repository.skillCatalog?.entries ?? [] }
    private var trash: [SkillAdminTrashEntry] { repository.skillCatalog?.trash ?? [] }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            if initialID != nil, repository.skillCatalog != nil, !openedInitialID {
                ContentUnavailableView(String(localized: "skills_unavailable"), systemImage: "link", description: Text(initialSkillError ?? String(localized: "skills_local_missing")))
            }
            HStack {
                Button("settings_parity_refresh_catalog") { Task { await load() } }.disabled(!repository.connected)
                Button("settings_parity_create_skill") { target = nil; name = ""; markdown = ""; revision = nil; writable = true }
                    .disabled(!repository.connected)
            }
            ForEach(entries) { entry in
                Button {
                    target = entry.identity
                    writable = false
                    Task { await repository.admin(domain: "skill", action: "get_document", skillTarget: entry.identity) }
                } label: {
                    HStack { Text(entry.name.displayValue); Spacer(); Text(entry.source).foregroundStyle(.secondary) }
                }
            }
            if writable || target != nil {
                Picker(String(localized: "skills_source_label"), selection: $scope) { Text(String(localized: "settings_parity_user_layer")).tag("user"); Text(String(localized: "settings_parity_project_layer")).tag("project") }
                    .disabled(target != nil)
                TextField("settings_parity_skill_name", text: $name).textFieldStyle(.roundedBorder).disabled(!writable)
                TextEditor(text: $markdown).font(.system(.body, design: .monospaced)).frame(minHeight: 260)
                    .disabled(!writable || repository.saving).autocorrectionDisabled().textInputAutocapitalization(.never)
                if !writable { Label(String(localized: "skills_readonly_sources"), systemImage: "lock") }
                Button("settings_parity_save_skill") { Task { await save() } }
                    .disabled(!writable || repository.saving || name.isEmpty || markdown.isEmpty)
                if target != nil && writable {
                    Menu(String(localized: "skills_move")) {
                        Button("settings_parity_user_layer") { move(to: "user") }.disabled(scope == "user")
                        Button("settings_parity_project_layer") { move(to: "project") }.disabled(scope == "project")
                    }.disabled(repository.saving)
                    Button("settings_parity_skill_trash", role: .destructive) {
                        Task { await repository.admin(domain: "skill", action: "trash_skill", skillTarget: target, scope: scope, revision: revision, mutation: true) }
                    }.disabled(repository.saving)
                }
            }
            if !trash.isEmpty {
                Text(String(localized: "skills_trash_title")).font(.headline)
                ForEach(trash) { entry in
                    Button(String(localized: "skills_restore_name \(entry.name.displayValue)")) {
                        Task { await repository.admin(domain: "skill", action: "restore_skill", skillTarget: entry.identity,
                                                      scope: entry.source, mutation: true) }
                    }.disabled(repository.saving)
                }
            }
            LayeredSettingsPage(keys: ["syncClaudeAiSkills"])
        }.task(id: repository.sourceGeneration) { await load() }
        .onChange(of: repository.sourceGeneration) { _, _ in target = nil; markdown = ""; revision = nil; writable = false }
        .onChange(of: repository.skillCatalog) { _, catalog in
            openInitialSkill()
            if let target, catalog?.entries.contains(where: { $0.identity == target }) != true {
                self.target = nil; writable = false; markdown = ""; revision = nil
            }
        }
        .onChange(of: repository.skillDocument) { _, document in
            guard let document, document.matches(selectedIdentity: target) else { return }
            target = document.identity
            name = document.name.displayValue
            markdown = document.markdown
            scope = document.source
            revision = document.revision
            writable = document.writable
        }
    }
    private func load() async { await repository.admin(domain: "skill", action: "get_catalog"); openInitialSkill() }
    private func openInitialSkill() {
        guard !openedInitialID, let initialID else { return }
        let candidates = entries.filter { entry in
            (initialScope == nil || entry.source == initialScope) && (entry.identity.id == initialID || entry.identity.displayValue == initialID || entry.name.displayValue == initialID)
        }
        guard candidates.count == 1, let entry = candidates.first else {
            initialSkillError = candidates.count > 1 ? String(localized: "skills_link_ambiguous") : String(localized: "skills_local_missing")
            return
        }
        openedInitialID = true; target = entry.identity; writable = false
        Task { await repository.admin(domain: "skill", action: "get_document", skillTarget: entry.identity) }
    }
    private func move(to destination: String) {
        let id = target
        let expectedRevision = revision
        target = nil; writable = false
        Task { await repository.admin(domain: "skill", action: "move_skill", skillTarget: id, scope: destination, revision: expectedRevision, mutation: true) }
    }
    private func save() async {
        await repository.admin(domain: "skill", action: target == nil ? "create_skill" : "save_document", skillTarget: target, scope: scope, revision: revision,
                               payload: DesktopSettingsRepository.json(["name": name, "markdown": markdown]), mutation: true)
    }
}

private struct PluginConfigurationEditor: View {
    let key: String
    let layer: DesktopSettingsLayer
    @State private var repository = DesktopSettingsRepository.shared
    @State private var draft = "{}"
    @State private var original = "{}"
    @State private var expectedRevision: String?
    private var currentRevision: String? {
        let catalog = DesktopSettingsRepository.object(repository.catalogs["plugin"] ?? "{}") ?? [:]
        return (catalog["revisions"] as? [String: String])?[layer.rawValue]
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(key).font(.headline)
            TextEditor(text: $draft).font(.system(.body, design: .monospaced)).frame(minHeight: 150)
                .autocorrectionDisabled().textInputAutocapitalization(.never)
                .disabled(!repository.canEdit(key: key, layer: layer))
            if expectedRevision != currentRevision && draft != original {
                Text("settings_parity_external_change").font(.caption).foregroundStyle(.orange)
            }
            HStack {
                Button("settings_save") { save() }
                    .disabled(!repository.canEdit(key: key, layer: layer) || expectedRevision == nil || draft == original)
                Button("settings_parity_discard_draft") { reload() }
            }
        }.onAppear { reload() }
        .onChange(of: layer) { _, _ in reload() }
        .onChange(of: repository.sourceGeneration) { _, _ in reload() }
        .onChange(of: currentRevision) { _, _ in if draft == original { reload() } }
    }
    private func reload() {
        draft = DesktopSettingsRepository.json(repository.ownValue(key: key, layer: layer) ?? [:])
        original = draft
        expectedRevision = currentRevision
    }
    private func save() {
        guard let expectedRevision, let value = DesktopSettingsRepository.object(draft) else {
            repository.errorMessage = String(localized: "plugins_configuration_json_required"); return
        }
        Task {
            await repository.admin(domain: "plugin", action: "save_config", scope: layer.rawValue, revision: expectedRevision,
                                   payload: DesktopSettingsRepository.json(["scope": layer.rawValue, key: value]), mutation: true)
        }
    }
}

struct PluginSensitiveFields: View {
    let pluginID: String
    let schemaJSON: String?
    let readOnly: Bool
    let reconnect: (() async throws -> Void)?
    @State private var repository = PluginSecretRepository.shared
    private var fields: [String: [String: Any]] {
        let schema = DesktopSettingsRepository.object(schemaJSON ?? "{}") ?? [:]
        return (schema["fields"] as? [String: [String: Any]] ?? [:]).filter { $0.value["sensitive"] as? Bool == true }
    }
    var body: some View {
        if !fields.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(fields.keys.sorted(), id: \.self) { key in
                    PluginSecretField(pluginID: pluginID, fieldKey: key, title: fields[key]?["title"] as? String ?? key,
                                      help: fields[key]?["description"] as? String ?? "", readOnly: readOnly)
                }
                if let message = repository.message { Text(message).font(.caption).foregroundStyle(.secondary) }
                if let error = repository.errorMessage { Text(error).font(.caption).foregroundStyle(.red) }
                if repository.needsReconnect {
                    Text(String(localized: "plugins_secret_reconnect_hint"))
                        .font(.caption).foregroundStyle(.secondary)
                    if let reconnect {
                        Button("settings_parity_reconnect") { Task { await repository.reconnect(using: reconnect) } }
                            .disabled(repository.busy)
                    }
                }
                Button("settings_parity_refresh_status") { Task { await repository.refresh() } }.disabled(repository.busy)
            }.task { await repository.refresh() }
        }
    }
}

private struct PluginSecretField: View {
    let pluginID: String
    let fieldKey: String
    let title: String
    let help: String
    let readOnly: Bool
    @State private var repository = PluginSecretRepository.shared
    @State private var settings = DesktopSettingsRepository.shared
    @State private var secret = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            LabeledContent(title, value: status)
            if !help.isEmpty { Text(help).font(.caption).foregroundStyle(.secondary) }
            SecureField("settings_parity_plugin_secret", text: $secret).textFieldStyle(.roundedBorder)
                .autocorrectionDisabled().textInputAutocapitalization(.never)
                .disabled(readOnly || repository.busy)
            HStack {
                Button("settings_save") {
                    let value = secret
                    secret = ""
                    Task { await repository.save(plugin: pluginID, key: fieldKey, secret: value) }
                }.disabled(readOnly || repository.busy || secret.isEmpty)
                Button(String(localized: "plugins_secret_delete"), role: .destructive) {
                    secret = ""
                    Task { await repository.delete(plugin: pluginID, key: fieldKey) }
                }.disabled(readOnly || repository.busy || repository.isConfigured(plugin: pluginID, key: fieldKey) != true)
            }
        }.onDisappear { secret = "" }
        .onChange(of: pluginID) { _, _ in secret = "" }
        .onChange(of: fieldKey) { _, _ in secret = "" }
        .onChange(of: settings.sourceGeneration) { _, _ in secret = "" }
    }
    private var status: String {
        switch repository.isConfigured(plugin: pluginID, key: fieldKey) {
        case true: String(localized: "settings_parity_secret_configured") + " · ••••••••"
        case false: String(localized: "settings_parity_secret_missing")
        case nil: String(localized: "settings_parity_secret_unknown")
        }
    }
}
