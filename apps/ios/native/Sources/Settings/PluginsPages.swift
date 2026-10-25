import SwiftUI

/// The Local App Builder has its own icon; every other plugin shows the plug.
func pluginIcon(_ id: String) -> LXIconName {
    id.split(separator: "@", maxSplits: 1).first == "local-app-builder" ? .appBuilder : .plug
}

/// "vendor · version", leaving out what is not known.
func pluginSubtitle(vendor: String?, version: String) -> String {
    [vendor, version].compactMap { nonBlank($0) }.joined(separator: " · ")
}

/// The directory a plugin's marketplace is cloned to, for the marketplace's `owner`.
private func marketplaceDir(_ catalog: PluginCatalog, _ plugin: InstalledPlugin) -> URL? {
    catalog.marketplace(plugin.marketplace)?.installLocation.map { URL(filePath: $0) }
}

private struct PluginOperationError: LocalizedError {
    let errorDescription: String?
    init(_ message: String) { errorDescription = message }
}

extension DesktopSettingsRepository {
    var pluginCatalog: PluginCatalog { PluginCatalog.parse(catalogs["plugin"]) }

    /// Asks for the plugin catalog and waits (up to 30 s) for it to arrive, changed or not.
    func reloadPluginCatalog() async {
        let before = catalogArrivals["plugin"]
        await admin(domain: "plugin", action: "get_catalog")
        for _ in 0 ..< 300 where connected && catalogArrivals["plugin"] == before {
            try? await Task.sleep(for: .milliseconds(100))
        }
    }

    /// One operation of the plugin system in the user layer, the way the plugin
    /// system defines it: applied with `confirmed` once the user agreed (the
    /// pages ask first), awaited, then the catalog is read again. Throws the
    /// engine's message.
    func applyPluginOperation(_ payload: [String: Any]) async throws {
        guard connected else { throw PluginOperationError(String(localized: "settings_parity_disconnected")) }
        guard let revision = pluginCatalog.revisions["user"] else {
            throw PluginOperationError(String(localized: "plugins_catalog_refresh_first"))
        }
        var body = payload
        body["scope"] = "user"
        body["confirmed"] = true
        let generation = sourceGeneration
        // Installing and updating fetch from the network; give them as long as Android does.
        await admin(domain: "plugin", action: "apply_operation", scope: "user", revision: revision,
                    payload: Self.json(body), mutation: true, timeout: .seconds(120))
        do {
            try await waitForConfirmation(generation: generation)
        } catch {
            await reloadPluginCatalog()
            throw error
        }
        await reloadPluginCatalog()
    }
}

/// What a page asks before a plugin operation that installs, removes or fetches.
private struct PluginConfirmation {
    let title: String
    let body: String
    let confirm: String
    let destructive: Bool
    let run: () -> Void
}

private extension View {
    func pluginConfirmation(_ confirmation: Binding<PluginConfirmation?>) -> some View {
        alert(
            confirmation.wrappedValue?.title ?? "",
            isPresented: Binding(get: { confirmation.wrappedValue != nil }, set: { if !$0 { confirmation.wrappedValue = nil } }),
            presenting: confirmation.wrappedValue
        ) { item in
            Button(item.confirm, role: item.destructive ? .destructive : nil) { item.run() }
            Button(String(localized: "common_cancel"), role: .cancel) {}
        } message: { item in
            Text(item.body)
        }
    }
}

/// A plugin's icon tile, as large as `size`.
private struct PluginIconTile: View {
    @Environment(\.theme) private var t
    let id: String
    let size: CGFloat
    var body: some View {
        LXIcon(name: pluginIcon(id), size: size * 0.5, color: t.accent, stroke: 1.8)
            .frame(width: size, height: size)
            .background(t.accent.mix(with: t.surface, amount: 0.16))
            .clipShape(.rect(cornerRadius: size * 0.26))
            .overlay { RoundedRectangle(cornerRadius: size * 0.26).stroke(t.accent.tint(0.28), lineWidth: 0.5) }
    }
}

private struct PluginPill: View {
    @Environment(\.theme) private var t
    let title: String
    let enabled: Bool
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Text(title).font(.footnote.weight(.semibold))
                .foregroundColor(enabled ? t.accent : t.text4)
                .padding(.horizontal, 14).padding(.vertical, 6)
                .background(t.accent.opacity(enabled ? 0.12 : 0.05))
                .clipShape(Capsule())
        }
        .disabled(!enabled)
    }
}

/// The plugin switch: the engine's state, changed only through an operation.
private struct PluginSwitch: View {
    let on: Bool
    let enabled: Bool
    let change: (Bool) -> Void
    var body: some View {
        LXToggle(isOn: Binding(get: { on }, set: { change($0) }))
            .allowsHitTesting(enabled)
            .opacity(enabled ? 1 : 0.5)
    }
}

private struct PluginNoteRow: View {
    @Environment(\.theme) private var t
    let text: String
    var body: some View {
        Text(text).font(.footnote).foregroundColor(t.text3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.vertical, 12)
    }
}

/// Settings → Plugins: the installed plugins with their switch, the plugins the
/// added marketplaces offer, and the marketplaces themselves. Every change is an
/// operation of the plugin system in the user layer; the raw per-layer
/// configuration stays one tap away under Advanced.
struct PluginsPage: View {
    let host: SettingsHost
    @Environment(\.theme) private var t
    @State private var repository = DesktopSettingsRepository.shared
    @State private var vendors: [String: String] = [:]
    @State private var busy: String?
    @State private var error: String?
    @State private var confirmation: PluginConfirmation?
    @State private var addingMarketplace = false
    @State private var marketplaceSource = ""

    private var catalog: PluginCatalog { repository.pluginCatalog }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            blurb(String(localized: "plugins_blurb"))
            if !repository.connected {
                Label("settings_parity_disconnected", systemImage: "network.slash").font(.footnote).foregroundColor(t.text3)
            } else if repository.catalogs["plugin"] == nil {
                ProgressView().frame(maxWidth: .infinity).padding(24)
            } else {
                content
            }
        }
        .task(id: repository.sourceGeneration) {
            if repository.connected { await repository.admin(domain: "plugin", action: "get_catalog") }
        }
        .task(id: repository.catalogs["plugin"]) {
            let catalog = catalog
            vendors = await Task.detached {
                var vendors: [String: String] = [:]
                for plugin in catalog.installed {
                    vendors[plugin.id] = PluginDetailsReader.vendor(
                        pluginDir: URL(filePath: plugin.path), marketplaceDir: marketplaceDir(catalog, plugin)
                    )
                }
                return vendors
            }.value
        }
        .pluginConfirmation($confirmation)
        .alert(String(localized: "plugins_marketplace_add"), isPresented: $addingMarketplace) {
            TextField(String(localized: "plugins_marketplace_add_hint"), text: $marketplaceSource)
                .autocorrectionDisabled().textInputAutocapitalization(.never)
            Button(String(localized: "plugins_marketplace_add")) {
                let source = marketplaceSource.trimmingCharacters(in: .whitespacesAndNewlines)
                marketplaceSource = ""
                guard !source.isEmpty else { return }
                run("marketplace:add", ["action": "marketplace_add", "source": source])
            }
            Button(String(localized: "common_cancel"), role: .cancel) { marketplaceSource = "" }
        } message: {
            Text("plugins_marketplace_add_body")
        }
    }

    @ViewBuilder private var content: some View {
        if let error {
            Text(String(localized: "plugins_operation_failed \(error)"))
                .font(.footnote).foregroundColor(t.danger).padding(.bottom, 12)
        }

        SettingsSection(label: String(localized: "plugins_section_installed")) {
            if catalog.installed.isEmpty { PluginNoteRow(text: String(localized: "plugins_installed_empty")) }
            ForEach(catalog.installed) { plugin in
                SettingsRow(
                    icon: pluginIcon(plugin.id),
                    label: plugin.displayName,
                    sub: pluginSubtitle(vendor: vendors[plugin.id], version: plugin.version),
                    chevron: false,
                    isLast: plugin.id == catalog.installed.last?.id,
                    onTap: { host.push(.pluginDetail(plugin.id)) }
                ) {
                    if busy == plugin.id {
                        ProgressView().controlSize(.small)
                    } else {
                        PluginSwitch(on: catalog.isEnabled(plugin), enabled: busy == nil) { on in
                            run(plugin.id, ["action": on ? "enable" : "disable", "plugin": plugin.id])
                        }
                    }
                }
                .accessibilityIdentifier("settings.plugins.installed.\(plugin.id)")
            }
        }

        let installable = catalog.installable
        SettingsSection(label: String(localized: "plugins_section_available")) {
            if installable.isEmpty {
                PluginNoteRow(text: String(localized: catalog.marketplaces.isEmpty ? "plugins_marketplaces_empty" : "plugins_available_empty"))
            }
            ForEach(installable) { plugin in
                SettingsRow(
                    icon: pluginIcon(plugin.id),
                    label: plugin.name,
                    sub: [plugin.marketplace, plugin.description].compactMap { nonBlank($0) }.joined(separator: " · "),
                    chevron: false,
                    isLast: plugin.id == installable.last?.id
                ) {
                    if busy == plugin.id {
                        ProgressView().controlSize(.small)
                    } else {
                        PluginPill(title: String(localized: "plugins_action_install"), enabled: busy == nil) {
                            confirmation = PluginConfirmation(
                                title: String(localized: "plugins_install_title \(plugin.name)"),
                                body: String(localized: "plugins_install_body"),
                                confirm: String(localized: "plugins_action_install"),
                                destructive: false
                            ) { run(plugin.id, ["action": "install", "plugin": plugin.id]) }
                        }
                    }
                }
                .accessibilityIdentifier("settings.plugins.available.\(plugin.id)")
            }
        }

        SettingsSection(label: String(localized: "plugins_section_marketplaces")) {
            ForEach(catalog.marketplaces) { marketplace in
                let key = "marketplace:\(marketplace.name)"
                SettingsRow(
                    icon: .globe,
                    label: marketplace.name,
                    sub: nonBlank(marketplace.sourceLocation),
                    chevron: false
                ) {
                    if busy == key {
                        ProgressView().controlSize(.small)
                    } else {
                        Menu {
                            Button(String(localized: "plugins_marketplace_update")) {
                                run(key, ["action": "marketplace_update", "name": marketplace.name])
                            }
                            Button(String(localized: "plugins_marketplace_remove"), role: .destructive) {
                                confirmation = PluginConfirmation(
                                    title: String(localized: "plugins_marketplace_remove_title \(marketplace.name)"),
                                    body: String(localized: "plugins_marketplace_remove_body"),
                                    confirm: String(localized: "plugins_marketplace_remove"),
                                    destructive: true
                                ) { run(key, ["action": "marketplace_remove", "name": marketplace.name]) }
                            }
                        } label: {
                            Text("⋯").font(.body).foregroundColor(busy == nil ? t.text2 : t.text4)
                                .padding(.horizontal, 8).padding(.vertical, 2)
                        }
                        .disabled(busy != nil)
                    }
                }
                .accessibilityIdentifier("settings.plugins.marketplace.\(marketplace.name)")
            }
            SettingsRow(
                icon: .plus,
                label: String(localized: "plugins_marketplace_add"),
                isLast: true,
                onTap: busy == nil ? { addingMarketplace = true } : nil
            ) {
                if busy == "marketplace:add" { ProgressView().controlSize(.small) }
            }
            .accessibilityIdentifier("settings.plugins.marketplace.add")
        }

        SettingsSection {
            SettingsRow(
                icon: .cog,
                label: String(localized: "plugins_advanced"),
                sub: String(localized: "plugins_advanced_sub"),
                isLast: true,
                onTap: { host.push(.pluginsAdvanced) }
            )
            .accessibilityIdentifier("settings.plugins.advanced")
        }
    }

    private func run(_ key: String, _ payload: [String: Any]) {
        busy = key
        error = nil
        Task {
            defer { busy = nil }
            do {
                try await repository.applyPluginOperation(payload)
            } catch is CancellationError {
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}

/// One installed plugin: who made it, what it contains (read from its own
/// files), its switch and secrets, and update and uninstall.
struct PluginDetailPage: View {
    let host: SettingsHost
    let pluginID: String
    @Environment(\.theme) private var t
    @State private var repository = DesktopSettingsRepository.shared
    @State private var details = PluginDetails()
    @State private var busy = false
    @State private var error: String?
    @State private var notice: String?
    @State private var confirmation: PluginConfirmation?

    private var catalog: PluginCatalog { repository.pluginCatalog }

    var body: some View {
        Group {
            if let plugin = catalog.installed(pluginID) {
                content(plugin)
            } else if repository.connected && repository.catalogs["plugin"] == nil {
                ProgressView().frame(maxWidth: .infinity).padding(24)
            } else {
                ContentUnavailableView(String(localized: "plugins_unavailable"), systemImage: "link", description: Text(String(localized: "plugins_local_missing")))
            }
        }
        .task(id: repository.sourceGeneration) {
            if repository.connected && repository.catalogs["plugin"] == nil {
                await repository.admin(domain: "plugin", action: "get_catalog")
            }
        }
        .task(id: repository.catalogs["plugin"]) {
            let catalog = catalog
            guard let plugin = catalog.installed(pluginID) else { details = PluginDetails(); return }
            details = await Task.detached {
                PluginDetailsReader.read(pluginDir: URL(filePath: plugin.path), marketplaceDir: marketplaceDir(catalog, plugin))
            }.value
        }
        .pluginConfirmation($confirmation)
    }

    @ViewBuilder private func content(_ plugin: InstalledPlugin) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 14) {
                PluginIconTile(id: plugin.id, size: 56)
                VStack(alignment: .leading, spacing: 3) {
                    Text(plugin.displayName).font(.body.weight(.semibold)).foregroundColor(t.text)
                    Text(pluginSubtitle(vendor: details.vendor, version: plugin.version)).font(.footnote).foregroundColor(t.text3)
                }
            }
            .padding(.bottom, 14)
            if !plugin.description.isEmpty {
                Text(plugin.description).font(.subheadline).foregroundColor(t.text2).lineSpacing(4).padding(.bottom, 16)
            }
            if let error {
                Text(String(localized: "plugins_operation_failed \(error)"))
                    .font(.footnote).foregroundColor(t.danger).padding(.bottom, 12)
            }
            if let notice {
                Text(notice).font(.footnote).foregroundColor(t.text3).padding(.bottom, 12)
            }

            SettingsSection {
                SettingsRow(label: String(localized: "plugins_enabled"), chevron: false, isLast: true) {
                    PluginSwitch(on: catalog.isEnabled(plugin), enabled: !busy) { on in
                        perform { try await repository.applyPluginOperation(["action": on ? "enable" : "disable", "plugin": plugin.id]) }
                    }
                }
            }

            let info = infoRows(plugin)
            if !info.isEmpty {
                SettingsSection(label: String(localized: "plugins_detail_about")) {
                    ForEach(Array(info.enumerated()), id: \.offset) { index, row in
                        SettingsRow(label: row.0, value: row.1, chevron: false, isLast: index == info.count - 1)
                    }
                }
            }
            componentSection(String(localized: "plugins_detail_skills"), details.skills)
            componentSection(String(localized: "plugins_detail_commands"), details.commands)
            componentSection(String(localized: "plugins_detail_agents"), details.agents)
            componentSection(String(localized: "plugins_detail_hooks"), details.hooks.map { PluginComponent(name: $0, description: "") })
            componentSection(String(localized: "plugins_detail_mcp"), details.mcpServers.map { PluginComponent(name: $0, description: "") })

            if hasSecrets(plugin) {
                SettingsSection(label: String(localized: "plugins_detail_settings")) {
                    PluginSensitiveFields(
                        pluginID: plugin.id,
                        schemaJSON: plugin.configSchemaJSON,
                        readOnly: !repository.canEdit(key: "pluginConfigs", layer: .user),
                        reconnect: host.onReconnectAfterSecretChange
                    )
                    .padding(14)
                }
            }

            SettingsSection {
                if let marketplace = plugin.marketplace {
                    SettingsRow(
                        label: String(localized: "plugins_action_update"),
                        chevron: false,
                        onTap: busy ? nil : { update(plugin, from: marketplace) }
                    ) {
                        if busy { ProgressView().controlSize(.small) }
                    }
                    .accessibilityIdentifier("settings.plugin.update")
                }
                SettingsRow(
                    label: String(localized: "plugins_action_uninstall"),
                    chevron: false,
                    danger: true,
                    isLast: true,
                    onTap: busy ? nil : {
                        confirmation = PluginConfirmation(
                            title: String(localized: "plugins_uninstall_title \(plugin.displayName)"),
                            body: String(localized: "plugins_uninstall_body"),
                            confirm: String(localized: "plugins_action_uninstall"),
                            destructive: true
                        ) {
                            perform {
                                try await repository.applyPluginOperation(["action": "uninstall", "plugin": plugin.id])
                                host.pop()
                            }
                        }
                    }
                )
                .accessibilityIdentifier("settings.plugin.uninstall")
            }
        }
    }

    private func infoRows(_ plugin: InstalledPlugin) -> [(String, String)] {
        var rows: [(String, String)] = []
        if let vendor = details.vendor { rows.append((String(localized: "plugins_detail_vendor"), vendor)) }
        if !plugin.version.isEmpty { rows.append((String(localized: "plugins_detail_version"), plugin.version)) }
        if let marketplace = plugin.marketplace { rows.append((String(localized: "plugins_detail_marketplace"), marketplace)) }
        if let license = details.license { rows.append((String(localized: "plugins_detail_license"), license)) }
        if let homepage = details.homepage { rows.append((String(localized: "plugins_detail_homepage"), homepage)) }
        if let repository = details.repository { rows.append((String(localized: "plugins_detail_repository"), repository)) }
        if !plugin.dependencies.isEmpty {
            rows.append((String(localized: "plugins_detail_dependencies"), plugin.dependencies.joined(separator: ", ")))
        }
        return rows
    }

    @ViewBuilder private func componentSection(_ label: String, _ components: [PluginComponent]) -> some View {
        if !components.isEmpty {
            SettingsSection(label: label) {
                ForEach(Array(components.enumerated()), id: \.offset) { index, component in
                    SettingsRow(label: component.name, sub: nonBlank(component.description), chevron: false,
                                isLast: index == components.count - 1)
                }
            }
        }
    }

    private func hasSecrets(_ plugin: InstalledPlugin) -> Bool {
        let schema = DesktopSettingsRepository.object(plugin.configSchemaJSON) ?? [:]
        let fields = schema["fields"] as? [String: [String: Any]] ?? [:]
        return fields.values.contains { $0["sensitive"] as? Bool == true }
    }

    private func update(_ plugin: InstalledPlugin, from marketplace: String) {
        perform {
            // A marketplace only learns of new versions when it is fetched again.
            try await repository.applyPluginOperation(["action": "marketplace_update", "name": marketplace])
            try await repository.applyPluginOperation(["action": "update", "plugin": plugin.id])
            if repository.pluginCatalog.installed(plugin.id)?.version == plugin.version {
                notice = String(localized: "plugins_up_to_date")
            }
        }
    }

    private func perform(_ block: @escaping @MainActor () async throws -> Void) {
        busy = true
        error = nil
        notice = nil
        Task {
            defer { busy = false }
            do {
                try await block()
            } catch is CancellationError {
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}
