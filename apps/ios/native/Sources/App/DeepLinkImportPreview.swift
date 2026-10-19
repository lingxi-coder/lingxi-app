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
                    Section("Review \(kind) import") {
                        Text("Configuration from an external link. Review its contents before applying it. Commands and plugins can access your workspace when enabled.")
                        LabeledContent("Scope", value: scope)
                        Text(target.payloadJSON ?? "{}").font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                        if kind == "mcp", let name = payload["name"] as? String,
                           let raw = mcpScope["raw_json"] as? String,
                           let root = DesktopSettingsRepository.object(raw),
                           (root["mcpServers"] as? [String: Any] ?? root)[name] != nil {
                            Text("This replaces the server with the same name in the selected scope.").foregroundStyle(.orange)
                        }
                        if let pluginSummary { Text(pluginSummary) }
                    }
                    if let error { Section { Text(error).foregroundStyle(.red) } }
                    if let message = repository.errorMessage { Section { Text(message).foregroundStyle(.red) } }
                    Section {
                        Button("Import") { Task { await apply() } }
                            .disabled(importing || reviewGeneration == nil || repository.saving || (kind == "plugin" && pluginSummary == nil))
                            .accessibilityIdentifier("deeplink.import.confirm")
                        if importing { ProgressView("Waiting for engine confirmation…") }
                    }
                }
                .navigationTitle("Import preview")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() }.disabled(importing) } }
                .task { await prepareReview() }
                .onChange(of: repository.sourceGeneration) { _, _ in
                    reviewGeneration = nil; error = "The engine changed. Close this preview and open the link again."
                }
                .interactiveDismissDisabled(importing)
            }
        }
    }

    private func prepareReview() async {
        guard repository.connected else { error = "Connect the local engine before importing configuration."; return }
        guard ["user", "project", "local"].contains(scope), kind == "mcp" || scope == "user" || scope == "project" else {
            error = "This import scope is not writable on this device."; return
        }
        let generation = repository.sourceGeneration
        if kind == "mcp" {
            await repository.admin(domain: "mcp", action: "get_snapshot")
            for _ in 0..<100 where repository.catalogs["mcp"] == nil && generation == repository.sourceGeneration { try? await Task.sleep(for: .milliseconds(50)) }
            guard let revision = mcpScope["revision_sha256"] as? String else { error = "The engine did not load this MCP scope."; return }
            reviewedRevision = revision
        } else if kind == "plugin" {
            guard scope == "user", let plugin = payload["plugin"] as? String, !plugin.isEmpty else { error = "A plugin import requires a plugin ID in the user scope."; return }
            await repository.reloadPluginCatalog()
            guard let revision = repository.pluginCatalog.revisions[scope] else { error = "The plugin catalog did not load."; return }
            reviewedRevision = revision
            await repository.admin(domain: "plugin", action: "preview_operation", scope: scope, revision: revision,
                payload: DesktopSettingsRepository.json(["action": "install", "plugin": plugin, "scope": scope]), mutation: true)
            do { try await repository.waitForConfirmation(generation: generation) }
            catch { self.error = error.localizedDescription; return }
            pluginSummary = DesktopSettingsRepository.object(repository.documents["plugin"] ?? "{}")?["summary"] as? String
            guard pluginSummary != nil else { error = "The engine did not return a plugin preview."; return }
        } else if kind == "skill" {
            guard let name = payload["name"] as? String, !name.isEmpty, let markdown = payload["markdown"] as? String, !markdown.isEmpty else {
                error = "A skill import requires a name and markdown content."; return
            }
        }
        guard generation == repository.sourceGeneration else { error = "The engine changed during review."; return }
        reviewGeneration = generation
    }

    private func apply() async {
        guard let generation = reviewGeneration, generation == repository.sourceGeneration else { error = "The engine changed during review."; return }
        importing = true
        defer { importing = false }
        if kind == "mcp" {
            guard mcpScope["revision_sha256"] as? String == reviewedRevision,
                  let name = payload["name"] as? String, !name.isEmpty, let config = payload["config"] as? [String: Any] else {
                error = "MCP imports require name and config fields, and an unchanged scope. Open the link again to review."; return
            }
            await repository.admin(domain: "mcp", action: "save_server", target: name, scope: scope, revision: reviewedRevision,
                payload: DesktopSettingsRepository.json(["scope": scope, "name": name, "config": config]), mutation: true)
        } else if kind == "skill" {
            await repository.admin(domain: "skill", action: "create_skill", scope: scope,
                payload: DesktopSettingsRepository.json(["name": payload["name"] ?? "", "markdown": payload["markdown"] ?? ""]), mutation: true)
        } else if kind == "plugin" {
            guard repository.pluginCatalog.revisions[scope] == reviewedRevision else { error = "Plugin configuration changed. Open the link again to review."; return }
            await repository.admin(domain: "plugin", action: "apply_operation", scope: scope, revision: reviewedRevision,
                payload: DesktopSettingsRepository.json(["action": "install", "plugin": payload["plugin"] ?? "", "scope": scope, "confirmed": true]), mutation: true, timeout: .seconds(120))
        }
        do { try await repository.waitForConfirmation(generation: generation); dismiss() }
        catch { self.error = error.localizedDescription }
    }
}
