import SwiftUI

/// External configuration is inert until this review's explicit Import button.
struct DeepLinkImportPreview: View {
    let target: CanonicalDeepLink
    @Environment(\.dismiss) private var dismiss
    @State private var repository = DesktopSettingsRepository.shared
    @State private var importing = false
    @State private var error: String?
    @State private var reviewGeneration: Int?
    @State private var reviewedRevision: String?
    @State private var pluginSummary: String?

    private var kind: String { target.parts[1] }
    private var payload: [String: Any] { target.payload }
    private var scope: String { target.params["scope"] ?? "user" }
    private var mcpScope: [String: Any] {
        let snapshot = DesktopSettingsRepository.object(repository.catalogs["mcp"] ?? "{}") ?? [:]
        return (snapshot["scopes"] as? [[String: Any]] ?? []).first { $0["scope"] as? String == scope } ?? [:]
    }

    var body: some View {
        if kind == "provider", let layer = DesktopSettingsLayer(rawValue: scope), layer != .managed {
            ProviderBulkImportPage(layer: layer, initialInput: target.payloadJSON ?? "{}")
        } else {
            NavigationStack {
                Form {
                    Section("settings_link_import_review_confirm") {
                        Text(String(localized: "settings_link_import_intro"))
                        LabeledContent(String(localized: "cron_scope_label"), value: scope)
                        Text(target.payloadJSON ?? "{}").font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                        if kind == "mcp", let name = payload["name"] as? String,
                           let raw = mcpScope["raw_json"] as? String,
                           let root = DesktopSettingsRepository.object(raw),
                           (root["mcpServers"] as? [String: Any] ?? root)[name] != nil {
                            Text(String(localized: "settings_link_import_conflicts \(name)")).foregroundStyle(.orange)
                        }
                        if let pluginSummary { Text(pluginSummary) }
                    }
                    if let error { Section { Text(error).foregroundStyle(.red) } }
                    if let message = repository.errorMessage { Section { Text(message).foregroundStyle(.red) } }
                    Section {
                        Button(String(localized: "settings_link_import_confirm")) { Task { await apply() } }
                            .disabled(importing || reviewGeneration == nil || repository.saving || (kind == "plugin" && pluginSummary == nil))
                            .accessibilityIdentifier("deeplink.import.confirm")
                        if importing { ProgressView(String(localized: "settings_wait_engine_confirmation")) }
                    }
                }
                .navigationTitle(String(localized: "settings_link_import_preview_title"))
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() }.disabled(importing) } }
                .task { await prepareReview() }
                .onChange(of: repository.sourceGeneration) { _, _ in
                    reviewGeneration = nil; error = String(localized: "settings_link_import_workspace_changed")
                }
                .interactiveDismissDisabled(importing)
            }
        }
    }

    private func prepareReview() async {
        guard repository.connected else { error = String(localized: "settings_link_import_connect_engine"); return }
        guard ["user", "project", "local"].contains(scope), kind == "mcp" || scope == "user" || scope == "project" else {
            error = String(localized: "settings_link_import_scope_read_only"); return
        }
        let generation = repository.sourceGeneration
        if kind == "mcp" {
            await repository.admin(domain: "mcp", action: "get_snapshot")
            for _ in 0..<100 where repository.catalogs["mcp"] == nil && generation == repository.sourceGeneration { try? await Task.sleep(for: .milliseconds(50)) }
            guard let revision = mcpScope["revision_sha256"] as? String else { error = String(localized: "settings_link_import_wait_scope"); return }
            reviewedRevision = revision
        } else if kind == "plugin" {
            guard scope == "user", let plugin = payload["plugin"] as? String, !plugin.isEmpty else { error = String(localized: "settings_link_import_plugin_required"); return }
            await repository.reloadPluginCatalog()
            guard let revision = repository.pluginCatalog.revisions[scope] else { error = String(localized: "settings_link_import_wait_catalog"); return }
            reviewedRevision = revision
            await repository.admin(domain: "plugin", action: "preview_operation", scope: scope, revision: revision,
                payload: DesktopSettingsRepository.json(["action": "install", "plugin": plugin, "scope": scope]), mutation: true)
            do { try await repository.waitForConfirmation(generation: generation) }
            catch { self.error = error.localizedDescription; return }
            pluginSummary = DesktopSettingsRepository.object(repository.documents["plugin"] ?? "{}")?["summary"] as? String
            guard pluginSummary != nil else { error = String(localized: "settings_link_import_plugin_preview_missing"); return }
        } else if kind == "skill" {
            guard let name = payload["name"] as? String, !name.isEmpty, let markdown = payload["markdown"] as? String, !markdown.isEmpty else {
                error = String(localized: "settings_link_import_skill_required"); return
            }
        }
        guard generation == repository.sourceGeneration else { error = String(localized: "settings_link_import_workspace_changed"); return }
        reviewGeneration = generation
    }

    private func apply() async {
        guard let generation = reviewGeneration, generation == repository.sourceGeneration else { error = String(localized: "settings_link_import_workspace_changed"); return }
        importing = true
        defer { importing = false }
        if kind == "mcp" {
            guard mcpScope["revision_sha256"] as? String == reviewedRevision,
                  let name = payload["name"] as? String, !name.isEmpty, let config = payload["config"] as? [String: Any] else {
                error = String(localized: "settings_link_import_mcp_review_again"); return
            }
            await repository.admin(domain: "mcp", action: "save_server", target: name, scope: scope, revision: reviewedRevision,
                payload: DesktopSettingsRepository.json(["scope": scope, "name": name, "config": config]), mutation: true)
        } else if kind == "skill" {
            await repository.admin(domain: "skill", action: "create_skill", scope: scope,
                payload: DesktopSettingsRepository.json(["name": payload["name"] ?? "", "markdown": payload["markdown"] ?? ""]), mutation: true)
        } else if kind == "plugin" {
            guard repository.pluginCatalog.revisions[scope] == reviewedRevision else { error = String(localized: "settings_link_import_config_changed"); return }
            await repository.admin(domain: "plugin", action: "apply_operation", scope: scope, revision: reviewedRevision,
                payload: DesktopSettingsRepository.json(["action": "install", "plugin": payload["plugin"] ?? "", "scope": scope, "confirmed": true]), mutation: true, timeout: .seconds(120))
        }
        do { try await repository.waitForConfirmation(generation: generation); dismiss() }
        catch { self.error = error.localizedDescription }
    }
}
