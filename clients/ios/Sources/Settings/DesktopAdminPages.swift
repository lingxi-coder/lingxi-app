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
            Picker("Settings layer", selection: $layer) {
                ForEach(DesktopSettingsLayer.allCases) { Text($0.title).tag($0) }
            }.pickerStyle(.segmented)
            Text("Hooks run commands around engine events. Review the selected layer before saving.").font(.footnote)
            TextEditor(text: $draft).font(.system(.body, design: .monospaced)).frame(minHeight: 260)
                .autocorrectionDisabled().textInputAutocapitalization(.never).disabled(!writable)
            HStack {
                Button("Reload") { Task { await load() } }.disabled(!repository.connected)
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
        guard let value = DesktopSettingsRepository.object(draft) else { repository.errorMessage = "Hooks must be a JSON object."; return }
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
            Button("Refresh plugins and marketplaces") { Task { await load() } }.disabled(!repository.connected)
            SettingsLayerPicker(selection: $layer)
            ForEach(installed.compactMap { $0["id"] as? String }, id: \.self) { id in
                let row = installed.first { $0["id"] as? String == id }
                GroupBox(row?["display_name"] as? String ?? id) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(row?["version"] as? String ?? "")
                        PluginSensitiveFields(pluginID: id, schemaJSON: row?["config_schema_json"] as? String,
                                              readOnly: !repository.canEdit(key: "pluginConfigs", layer: layer), reconnect: host.onReconnectAfterSecretChange)
                        HStack {
                            Button("Enable") { preview(action: "enable", target: ["plugin": id]) }
                            Button("Disable") { preview(action: "disable", target: ["plugin": id]) }
                            if row?["source"] as? String != "builtin" {
                                Button("Update") { preview(action: "update", target: ["plugin": id]) }
                                Button("Uninstall", role: .destructive) { preview(action: "uninstall", target: ["plugin": id]) }
                            }
                        }.disabled(!writable)
                    }
                }
            }
            ForEach(marketplaces.compactMap { $0["name"] as? String }, id: \.self) { name in
                HStack {
                    Label(name, systemImage: "shippingbox")
                    Spacer()
                    Button("Update") { preview(action: "marketplace_update", target: ["name": name]) }
                    Button("settings_parity_remove", role: .destructive) { preview(action: "marketplace_remove", target: ["name": name]) }
                }.disabled(!writable)
            }
            if repository.catalogs["plugin"] != nil && installed.isEmpty { Text("No installed plugins.").foregroundStyle(.secondary) }
            ForEach(available.compactMap { $0["id"] as? String }, id: \.self) { id in
                HStack {
                    Text(id)
                    Spacer()
                    Button("Install / update") { preview(action: available.first { $0["id"] as? String == id }?["installed"] as? Bool == true ? "update" : "install", target: ["plugin": id]) }.disabled(!writable)
                }
            }
            TextField("settings_parity_marketplace_source", text: $marketplaceSource).textFieldStyle(.roundedBorder)
            Button("settings_parity_preview_marketplace") { preview(action: "marketplace_add", target: ["source": marketplaceSource]) }
                .disabled(!writable || marketplaceSource.isEmpty)
            if let previewSummary {
                GroupBox("Review operation") {
                    VStack(alignment: .leading, spacing: 10) {
                        Text(previewSummary).textSelection(.enabled)
                        Button("settings_parity_confirm_apply") { applyPreview() }.disabled(!writable || previewPayload == nil)
                        Button("common_cancel") { self.previewSummary = nil; previewPayload = nil }
                    }
                }
            }
            DisclosureGroup("Advanced plugin configuration") { ForEach(["enabledPlugins", "pluginConfigs", "extraKnownMarketplaces"], id: \.self) { PluginConfigurationEditor(key: $0, layer: layer) } }
            Button("Mobile app plugin") { host.push(.localAppPlugin) }
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
        guard let currentRevision else { repository.errorMessage = "Refresh the plugin catalog before changing it."; return }
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
struct DesktopMCPAdminPage: View {
    let host: SettingsHost
    @State private var repository = DesktopSettingsRepository.shared
    @State private var scope = "user"
    @State private var selectedName: String?
    @State private var name = ""
    @State private var draft = "{}"
    @State private var draftRevision: String?
    private var snapshot: [String: Any] { DesktopSettingsRepository.object(repository.catalogs["mcp"] ?? "{}") ?? [:] }
    private var scopeRow: [String: Any] {
        (snapshot["scopes"] as? [[String: Any]] ?? []).first { $0["scope"] as? String == scope } ?? [:]
    }
    private var servers: [String: Any] {
        let root = DesktopSettingsRepository.object(scopeRow["raw_json"] as? String ?? "{}") ?? [:]
        return root["mcpServers"] as? [String: Any] ?? root
    }
    private var managed: Bool { host.managedMcpInventory(serverName: name) != nil }
    private var writable: Bool { repository.connected && !repository.saving && draftRevision != nil && !managed }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            Picker("MCP storage scope", selection: $scope) {
                Text("User").tag("user"); Text("Local workspace").tag("local"); Text("Project .mcp.json").tag("project")
            }.pickerStyle(.segmented)
            Text(scopeRow["path"] as? String ?? "Load the engine configuration to view this scope.")
                .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            DisclosureGroup("Runtime servers") {
                ForEach(host.convo.mcpServers) { server in
                    HStack {
                        Text(server.name)
                        Spacer()
                        Text(String(describing: server.status)).foregroundStyle(.secondary)
                        if host.managedMcpInventory(serverName: server.id) != nil {
                            Button("Inventory") { host.push(.mcpEdit(server.id)) }
                        }
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
            if managed { Label("This app-managed server is read-only.", systemImage: "lock") }
            HStack {
                Button("settings_parity_save_server") { Task { await save(remove: false) } }.disabled(!writable || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                if selectedName != nil {
                    Button("settings_parity_remove_server", role: .destructive) { Task { await save(remove: true) } }.disabled(!writable)
                }
            }
        }.task(id: repository.sourceGeneration) { await load() }
        .onChange(of: scope) { _, _ in select(nil) }
        .onChange(of: repository.sourceGeneration) { _, _ in select(nil) }
        .onChange(of: repository.catalogs["mcp"]) { _, _ in select(selectedName.flatMap { servers[$0] == nil ? nil : $0 }) }
    }
    private func load() async { await repository.admin(domain: "mcp", action: "get_snapshot") }
    private func select(_ selected: String?) {
        selectedName = selected
        name = selected ?? ""
        draft = DesktopSettingsRepository.json(selected.flatMap { servers[$0] } ?? [:])
        draftRevision = scopeRow["revision_sha256"] as? String
    }
    private func save(remove: Bool) async {
        guard writable, let config = DesktopSettingsRepository.object(draft) else { repository.errorMessage = "MCP configuration must be a JSON object."; return }
        let payload: [String: Any] = ["scope": scope, "name": name, "config": config]
        await repository.admin(domain: "mcp", action: remove ? "remove_server" : "save_server", target: name, scope: scope,
                               revision: draftRevision, payload: DesktopSettingsRepository.json(payload), mutation: true)
    }
}

struct DesktopSkillsAdminPage: View {
    @State private var repository = DesktopSettingsRepository.shared
    @State private var target: String?
    @State private var scope = "user"
    @State private var name = ""
    @State private var markdown = ""
    @State private var revision: String?
    @State private var writable = false
    private var catalog: [String: Any] { DesktopSettingsRepository.object(repository.catalogs["skill"] ?? "{}") ?? [:] }
    private var entries: [[String: Any]] { catalog["entries"] as? [[String: Any]] ?? [] }
    private var trash: [[String: Any]] { catalog["trash"] as? [[String: Any]] ?? [] }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SettingsConnectionStatus()
            HStack {
                Button("settings_parity_refresh_catalog") { Task { await load() } }.disabled(!repository.connected)
                Button("settings_parity_create_skill") { target = nil; name = ""; markdown = ""; revision = nil; writable = true }
                    .disabled(!repository.connected)
            }
            ForEach(entries.compactMap { $0["id"] as? String }, id: \.self) { id in
                let row = entries.first { $0["id"] as? String == id }
                Button {
                    target = id
                    writable = false
                    Task { await repository.admin(domain: "skill", action: "get_document", target: id) }
                } label: {
                    HStack { Text(row?["name"] as? String ?? id); Spacer(); Text(row?["source"] as? String ?? "").foregroundStyle(.secondary) }
                }
            }
            if writable || target != nil {
                Picker("Skill source", selection: $scope) { Text("User").tag("user"); Text("Project").tag("project") }
                    .disabled(target != nil)
                TextField("settings_parity_skill_name", text: $name).textFieldStyle(.roundedBorder).disabled(!writable)
                TextEditor(text: $markdown).font(.system(.body, design: .monospaced)).frame(minHeight: 260)
                    .disabled(!writable || repository.saving).autocorrectionDisabled().textInputAutocapitalization(.never)
                if !writable { Label("Managed, bundled and plugin skills are read-only.", systemImage: "lock") }
                Button("settings_parity_save_skill") { Task { await save() } }
                    .disabled(!writable || repository.saving || name.isEmpty || markdown.isEmpty)
                if target != nil && writable {
                    Menu("Move Skill") {
                        Button("settings_parity_user_layer") { move(to: "user") }.disabled(scope == "user")
                        Button("settings_parity_project_layer") { move(to: "project") }.disabled(scope == "project")
                    }.disabled(repository.saving)
                    Button("settings_parity_skill_trash", role: .destructive) {
                        Task { await repository.admin(domain: "skill", action: "trash_skill", target: target, scope: scope, revision: revision, mutation: true) }
                    }.disabled(repository.saving)
                }
            }
            if !trash.isEmpty {
                Text("Trash").font(.headline)
                ForEach(trash.compactMap { $0["trashId"] as? String }, id: \.self) { id in
                    Button("Restore \(trash.first { $0["trashId"] as? String == id }?["name"] as? String ?? id)") {
                        Task { await repository.admin(domain: "skill", action: "restore_skill", target: id,
                                                      scope: trash.first { $0["trashId"] as? String == id }?["source"] as? String ?? "user", mutation: true) }
                    }.disabled(repository.saving)
                }
            }
            LayeredSettingsPage(keys: ["syncClaudeAiSkills"])
        }.task(id: repository.sourceGeneration) { await load() }
        .onChange(of: repository.sourceGeneration) { _, _ in target = nil; markdown = ""; revision = nil; writable = false }
        .onChange(of: repository.catalogs["skill"]) { _, _ in
            if let target, !entries.contains(where: { $0["id"] as? String == target }) {
                self.target = nil; writable = false; markdown = ""; revision = nil
            }
        }
        .onChange(of: repository.documents["skill"]) { _, json in
            guard let json, let doc = DesktopSettingsRepository.object(json), doc["id"] as? String == target || target == nil else { return }
            target = doc["id"] as? String
            name = doc["name"] as? String ?? ""
            markdown = doc["markdown"] as? String ?? ""
            scope = doc["source"] as? String ?? "user"
            revision = doc["revision"] as? String
            writable = doc["writable"] as? Bool ?? false
        }
    }
    private func load() async { await repository.admin(domain: "skill", action: "get_catalog") }
    private func move(to destination: String) {
        let id = target
        let expectedRevision = revision
        target = nil; writable = false
        Task { await repository.admin(domain: "skill", action: "move_skill", target: id, scope: destination, revision: expectedRevision, mutation: true) }
    }
    private func save() async {
        await repository.admin(domain: "skill", action: target == nil ? "create_skill" : "save_document", target: target, scope: scope, revision: revision,
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
            repository.errorMessage = "Plugin configuration must be a JSON object."; return
        }
        Task {
            await repository.admin(domain: "plugin", action: "save_config", scope: layer.rawValue, revision: expectedRevision,
                                   payload: DesktopSettingsRepository.json(["scope": layer.rawValue, key: value]), mutation: true)
        }
    }
}

private struct PluginSensitiveFields: View {
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
                    Text("Reconnect the engine when its current work has finished. Existing engine connections may cache an older secret.")
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
                Button("Delete stored secret", role: .destructive) {
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
