import SwiftUI

private func localized(_ key: String) -> String {
    String(localized: String.LocalizationValue(stringLiteral: key))
}

private func digestSummary(_ value: String) -> String {
    guard value.count > 12 else { return value }
    return String(value.prefix(12))
}

private func statusTint(_ badge: LocalAppStatusBadge, theme: Palette) -> Color {
    switch badge.tintName {
    case "green":
        theme.ok
    case "orange":
        .orange
    default:
        theme.text3
    }
}

func allowsMcpConfigurationEditing(_ inventory: LocalAppManagedMcpInventory?) -> Bool {
    inventory == nil
}

private struct LocalAppStatusBadgeView: View {
    @Environment(\.theme) private var theme
    let badge: LocalAppStatusBadge

    var body: some View {
        Label(badge.label, systemImage: badge.systemImageName)
            .font(.caption.weight(.semibold))
            .foregroundStyle(statusTint(badge, theme: theme))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(statusTint(badge, theme: theme).opacity(0.12), in: Capsule())
            .accessibilityLabel(badge.accessibilityLabel)
    }
}

struct LocalAppPluginPage: View {
    @Environment(\.theme) private var t
    let host: SettingsHost

    private var descriptor: LocalAppBuiltinPluginDescriptor { host.localAppsStore.builtinPluginDescriptor }
    private var status: LocalAppBuiltinPluginStatus? { host.localAppsStore.builtinPluginStatus }
    private var inventory: LocalAppBuiltinPluginInventory? { host.localAppsStore.builtinPluginInventory }

    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: localized("local_apps_plugin_title")) {
                SettingsRow(
                    icon: .plug,
                    iconColor: Color(srgb: 0, 0.78, 0.55),
                    label: descriptor.displayName,
                    subView: AnyView(VStack(alignment: .leading, spacing: 6) {
                        Text("\(descriptor.pluginID) · v\(descriptor.version)")
                            .font(.system(size: 11.5, design: .monospaced))
                            .foregroundStyle(t.text4)
                        if let status {
                            LocalAppStatusBadgeView(badge: status.statusBadge)
                        }
                    }),
                    chevron: false,
                    isLast: true
                ) {
                    Toggle(
                        "",
                        isOn: Binding(
                            get: { host.localAppsStore.builtinPluginEffectiveEnabled },
                            set: { enabled in
                                Task { await host.localAppsStore.setBuiltinPluginEnabled(enabled) }
                            }
                        )
                    )
                    .labelsHidden()
                }
            }

            SettingsSection(label: "Inventory") {
                if let inventory {
                    SettingsRow(label: "Source", value: inventory.source, chevron: false)
                }
                SettingsRow(label: localized("local_apps_plugin_bundle_digest"), value: digestSummary(descriptor.archiveDigest), chevron: false)
                SettingsRow(label: "Version", value: descriptor.version, chevron: false)
                SettingsRow(label: String(format: String(localized: "local_apps_plugin_skills_count_fmt"), descriptor.skillCount), chevron: false)
                SettingsRow(label: String(format: String(localized: "local_apps_plugin_agents_count_fmt"), descriptor.agentCount), chevron: false)
                SettingsRow(label: String(format: String(localized: "local_apps_plugin_workflows_count_fmt"), descriptor.workflowCount), chevron: false)
                SettingsRow(label: String(format: String(localized: "local_apps_plugin_templates_count_fmt"), descriptor.templateCount), chevron: false, isLast: true)
            }

            SettingsSection(label: "Validation") {
                SettingsRow(
                    label: "Default enabled",
                    value: descriptor.defaultEnabled ? String(localized: "settings_status_on") : String(localized: "settings_status_off"),
                    chevron: false
                )
                SettingsRow(
                    label: localized("local_apps_plugin_validation_error"),
                    value: host.localAppsStore.builtinPluginCommandError ?? status?.validationError ?? String(localized: "common_none"),
                    chevron: false,
                    isLast: true
                )
            }
        }
    }
}

struct MCPListPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost

    private var managedInventories: [String: LocalAppManagedMcpInventory] {
        Dictionary(
            uniqueKeysWithValues: store.mcpServers.compactMap { server in
                host.managedMcpInventory(serverName: server.id).map { (server.id, $0) }
            }
        )
    }

    var body: some View {
        let connectedCount = store.mcpServers.filter { $0.status == .connected }.count
        let toolEvidenceKnown = store.mcpServers.allSatisfy {
            managedInventories[$0.id] != nil || $0.tools != nil
        }
        let knownTools = store.mcpServers.reduce(0) { partial, server in
            partial + (managedInventories[server.id]?.toolCount ?? server.tools ?? 0)
        }
        let toolCount = toolEvidenceKnown ? "\(knownTools)" : "—"

        VStack(spacing: 0) {
            Text("mcp_description_blurb")
                .font(.system(size: 11.5))
                .foregroundColor(t.text3)
                .lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.bottom, 14)

            HStack(spacing: 8) {
                statCard(String(localized: "mcp_stat_connected"), "\(connectedCount)")
                statCard(String(localized: "mcp_stat_available_tools"), toolCount)
            }
            .padding(.bottom, 18)

            SettingsSection(label: localized("local_apps_plugin_title")) {
                SettingsRow(
                    icon: .plug,
                    iconColor: Color(srgb: 0, 0.78, 0.55),
                    label: host.localAppsStore.builtinPluginDescriptor.displayName,
                    subView: AnyView(VStack(alignment: .leading, spacing: 6) {
                        Text("v\(host.localAppsStore.builtinPluginDescriptor.version) · \(digestSummary(host.localAppsStore.builtinPluginDescriptor.archiveDigest))")
                            .font(.system(size: 11.5, design: .monospaced))
                            .foregroundStyle(t.text4)
                        if let status = host.localAppsStore.builtinPluginStatus {
                            LocalAppStatusBadgeView(badge: status.statusBadge)
                        }
                    }),
                    value: "\(host.localAppsStore.builtinPluginDescriptor.skillCount)",
                    chevron: true,
                    isLast: true,
                    onTap: { host.push(.localAppPlugin) }
                )
            }

            SettingsSection(label: String(localized: "mcp_section_servers_count \(store.mcpServers.count)")) {
                ForEach(Array(store.mcpServers.enumerated()), id: \.element.id) { index, server in
                    if let inventory = managedInventories[server.id] {
                        managedServerRow(server: server, inventory: inventory, isLast: index == store.mcpServers.count - 1)
                    } else {
                        standardServerRow(server: server, isLast: index == store.mcpServers.count - 1)
                    }
                }
            }

            if store.mcpServers.isEmpty {
                Text(store.mcpListingLoaded ? String(localized: "mcp_empty_state") : String(localized: "mcp_loading_state"))
                    .font(.system(size: 12))
                    .foregroundColor(t.text4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.bottom, 12)
            }

            DashedAddButton(title: String(localized: "mcp_add_server"), action: host.addMcpServer)
            (Text("mcp_transport_description_prefix")
                + Text("mcp.directory").foregroundColor(t.accent))
                .font(.system(size: 11))
                .foregroundColor(t.text4)
                .lineSpacing(5)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 14)
        }
    }

    private func statCard(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label.uppercased()).font(.system(size: 10.5)).tracking(0.5).foregroundColor(t.text4)
            Text(value).font(.system(size: 18, weight: .bold)).foregroundColor(t.text)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }

    private func standardServerRow(server s: MCPServer, isLast: Bool) -> some View {
        let iconColor: Color = s.status == .connected ? Color(srgb: 0, 0.78, 0.55) : s.status == .error ? t.statusError : t.text4
        return SettingsRow(
            icon: .plug,
            iconColor: iconColor,
            label: s.name,
            subView: AnyView(VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 5) {
                    Circle().fill(s.status.dot(t)).frame(width: 6, height: 6)
                    Text("mcp_server_status_tools_detail \(s.status.label) \(s.tools.map(String.init) ?? "—") \(s.transportLabel)")
                        .font(.system(size: 11.5, design: .monospaced))
                        .foregroundColor(t.text4)
                }
                Text(s.endpointSummary)
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundColor(t.text4)
                    .lineLimit(1)
            }),
            chevron: false,
            isLast: isLast,
            onTap: { host.push(.mcpEdit(s.id)) }
        ) {
            LXToggle(isOn: toggle(s.id))
        }
    }

    private func managedServerRow(
        server: MCPServer,
        inventory: LocalAppManagedMcpInventory,
        isLast: Bool
    ) -> some View {
        SettingsRow(
            icon: .plug,
            iconColor: statusTint(inventory.publicationBadge, theme: t),
            label: inventory.serverName,
            subView: AnyView(VStack(alignment: .leading, spacing: 6) {
                Text("\(inventory.appName) · \(inventory.appID)")
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(t.text3)
                    .lineLimit(1)
                Text("\(inventory.buildID) · \(digestSummary(inventory.catalogDigest)) · \(inventory.toolCount)")
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundStyle(t.text4)
                    .lineLimit(1)
                HStack(spacing: 6) {
                    LocalAppStatusBadgeView(badge: inventory.publicationBadge)
                    LocalAppStatusBadgeView(badge: inventory.uiVerification.badge)
                    LocalAppStatusBadgeView(badge: inventory.mcpVerification.badge)
                }
            }),
            value: localized("local_apps_plugin_managed_mcp_source"),
            valueColor: t.text4,
            chevron: false,
            isLast: isLast,
            onTap: { host.push(.mcpEdit(server.id)) }
        )
    }

    private func toggle(_ id: String) -> Binding<Bool> {
        Binding(
            get: { store.mcpServers.first(where: { $0.id == id })?.enabled ?? false },
            set: { value in
                guard let index = store.mcpServers.firstIndex(where: { $0.id == id }) else { return }
                let previous = store.mcpServers[index].enabled
                store.mcpServers[index].enabled = value
                if !host.saveMcpServer(store.mcpServers[index]) {
                    store.mcpServers[index].enabled = previous
                }
            }
        )
    }
}

struct MCPEditPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let mcpId: String

    var body: some View {
        guard let server = store.mcpServers.first(where: { $0.id == mcpId }) else {
            DispatchQueue.main.async { host.pop() }
            return AnyView(EmptyView())
        }
        if let inventory = host.managedMcpInventory(serverName: mcpId) {
            return AnyView(ManagedLocalAppMCPEditPage(server: server, inventory: inventory, host: host))
        }
        let dot = server.status.dot(t)
        return AnyView(VStack(spacing: 0) {
            HStack(spacing: 8) {
                Circle().fill(dot).frame(width: 8, height: 8)
                    .overlay(Circle().stroke(dot.tint(0.22), lineWidth: 3).scaleEffect(1.75))
                Text("mcp_edit_status_tools_available \(server.status.label) \(server.tools.map(String.init) ?? "—")")
                    .font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
                Spacer()
                Button {
                    host.refreshMcp()
                } label: {
                    Text("mcp_reconnect")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(t.text2)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 6)
                        .background(t.windowBg)
                        .clipShape(RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(t.border, lineWidth: 0.5))
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(dot.mix(with: t.surface, amount: 0.08))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(dot.tint(0.24), lineWidth: 0.5))
            .padding(.bottom, 18)

            FieldLabel(text: String(localized: "settings_display_name"))
            SettingsField(text: bind(\.name), mono: false).padding(.bottom, 14)

            SettingsSection(label: String(localized: "mcp_section_configuration")) {
                SettingsRow(label: String(localized: "mcp_transport_mode"), chevron: false) {
                    Picker("", selection: bind(\.transport)) {
                        Text("HTTP").tag("http")
                        Text("SSE").tag("sse")
                        Text("WebSocket").tag("ws")
                        Text("stdio").tag("stdio")
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                }
                if server.transport == "stdio" {
                    SettingsRow(label: String(localized: "mcp_command"), chevron: false) {
                        SettingsField(text: bind(\.command), placeholder: "node", mono: true, trailingPadding: 0)
                            .frame(maxWidth: 180)
                    }
                    SettingsRow(label: String(localized: "mcp_arguments"), sub: String(localized: "mcp_arguments_hint"), chevron: false, isLast: true) {
                        SettingsField(text: argsBinding, placeholder: "--stdio", mono: true, trailingPadding: 0)
                            .frame(maxWidth: 180)
                    }
                } else {
                    SettingsRow(label: String(localized: "mcp_endpoint"), chevron: false, isLast: true) {
                        SettingsField(text: urlBinding, placeholder: "https://…", mono: true, trailingPadding: 0)
                            .frame(maxWidth: 220)
                    }
                }
            }
            FieldHint(String(localized: "mcp_endpoint_hint"))

            SettingsSection(label: String(localized: "mcp_section_transport_auth")) {
                SettingsRow(label: String(localized: "mcp_auth"), value: server.auth ?? String(localized: "common_none"), isLast: true, onTap: {})
            }
            SettingsSection(label: String(localized: "mcp_section_tool_permissions"), footer: String(localized: "mcp_tool_permissions_footer")) {
                if let tools = server.tools {
                    SettingsRow(label: String(localized: "mcp_tools_discovered_count \(tools)"), chevron: false, isLast: true) {
                        LXIcon(name: .check, size: 15, color: t.ok, stroke: 2.2)
                    }
                } else {
                    SettingsRow(label: String(localized: "mcp_tools_unavailable"), sub: String(localized: "mcp_tools_unavailable_sub"), chevron: false, isLast: true)
                }
            }
            SettingsSection {
                SettingsRow(label: String(localized: "mcp_enable_server"), chevron: false) {
                    LXToggle(isOn: Binding(get: { server.enabled }, set: { value in update { $0.enabled = value } }))
                }
            }
            Button { host.removeMcpServer(server) } label: {
                Text("mcp_remove_server")
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundColor(t.danger)
                    .frame(maxWidth: .infinity)
                    .padding(12)
                    .overlay(RoundedRectangle(cornerRadius: 11).stroke(t.border, lineWidth: 0.5))
            }
            .padding(.top, 8)
        })
    }

    private func bind(_ keyPath: WritableKeyPath<MCPServer, String>) -> Binding<String> {
        Binding(
            get: { store.mcpServers.first(where: { $0.id == mcpId })?[keyPath: keyPath] ?? "" },
            set: { value in update { $0[keyPath: keyPath] = value } }
        )
    }

    private var urlBinding: Binding<String> {
        Binding(
            get: { store.mcpServers.first(where: { $0.id == mcpId })?.url ?? "" },
            set: { value in update { $0.url = value } }
        )
    }

    private var argsBinding: Binding<String> {
        Binding(
            get: { store.mcpServers.first(where: { $0.id == mcpId })?.args.joined(separator: " ") ?? "" },
            set: { value in update { $0.args = value.split(whereSeparator: { $0.isWhitespace }).map(String.init) } }
        )
    }

    private func update(_ mutate: (inout MCPServer) -> Void) {
        guard allowsMcpConfigurationEditing(host.managedMcpInventory(serverName: mcpId)) else { return }
        if let index = store.mcpServers.firstIndex(where: { $0.id == mcpId }) {
            mutate(&store.mcpServers[index])
        }
    }
}

private struct ManagedLocalAppMCPEditPage: View {
    @Environment(\.theme) private var t
    let server: MCPServer
    let inventory: LocalAppManagedMcpInventory
    let host: SettingsHost

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header

                SettingsSection(label: localized("local_apps_mcp_inventory_title")) {
                    SettingsRow(label: localized("local_apps_mcp_server_name"), value: inventory.serverName, chevron: false)
                    SettingsRow(label: localized("local_apps_mcp_app_identity"), value: "\(inventory.appName) · \(inventory.appID)", chevron: false)
                    SettingsRow(label: localized("local_apps_mcp_build"), value: inventory.buildID, chevron: false)
                    SettingsRow(label: localized("local_apps_mcp_catalog_digest"), value: digestSummary(inventory.catalogDigest), chevron: false)
                    SettingsRow(label: localized("local_apps_mcp_surface_digest"), value: digestSummary(inventory.toolSurfaceDigest), chevron: false)
                    SettingsRow(label: localized("local_apps_mcp_authoring_revision"), value: "\(inventory.authoringRevision)", chevron: false, isLast: true)
                }

                SettingsSection(label: localized("local_apps_mcp_verification_title")) {
                    SettingsRow(label: localized("local_apps_mcp_ui_verification"), labelView: AnyView(LocalAppStatusBadgeView(badge: inventory.uiVerification.badge)), chevron: false)
                    SettingsRow(
                        label: localized("local_apps_mcp_mcp_verification"),
                        labelView: AnyView(LocalAppStatusBadgeView(badge: inventory.mcpVerification.badge)),
                        chevron: false,
                        isLast: true
                    )
                }

                SettingsSection(label: "\(localized("local_apps_mcp_tools_title")) \(inventory.toolCount)") {
                    ForEach(Array(inventory.tools.enumerated()), id: \.element.id) { index, tool in
                        SettingsRow(
                            label: tool.name,
                            sub: tool.title ?? tool.description ?? String(localized: "common_none"),
                            value: tool.ceilingSummary,
                            chevron: false,
                            isLast: index == inventory.tools.count - 1,
                            onTap: {}
                        )
                    }
                }

                ForEach(inventory.tools) { tool in
                    SettingsSection(label: "\(tool.name) · \(tool.ceilingSummary)") {
                        readOnlyToolField(label: localized("local_apps_mcp_tool_title"), value: tool.title ?? String(localized: "common_none"))
                        readOnlyToolField(label: localized("local_apps_mcp_tool_description"), value: tool.description ?? String(localized: "common_none"))
                        readOnlyToolField(label: localized("local_apps_mcp_tool_input_schema"), value: tool.inputSchemaSummary)
                        readOnlyToolField(label: localized("local_apps_mcp_tool_output_schema"), value: tool.outputSchemaSummary ?? String(localized: "common_none"))
                        readOnlyToolField(label: localized("local_apps_mcp_tool_annotations"), value: tool.annotationsSummary ?? String(localized: "common_none"))
                        readOnlyToolField(label: "Execution", value: tool.executionSummary ?? String(localized: "common_none"))
                        readOnlyToolField(label: localized("local_apps_mcp_tool_visible_meta"), value: tool.visibleMetaSummary ?? String(localized: "common_none"))
                        readOnlyToolField(label: localized("local_apps_mcp_tool_semantic_flow"), value: tool.semanticFlowSummary)
                    }
                }
            }
            .padding(.top, 8)
        }
        .safeAreaInset(edge: .bottom) {
            Text("Managed Local App MCP servers are read-only here.")
                .font(.caption)
                .foregroundStyle(t.text4)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)
                .background(.bar)
        }
    }

    private var header: some View {
        let dot = server.status.dot(t)
        return HStack(spacing: 8) {
            Circle().fill(dot).frame(width: 8, height: 8)
                .overlay(Circle().stroke(dot.tint(0.22), lineWidth: 3).scaleEffect(1.75))
            Text("mcp_edit_status_tools_available \(server.status.label) \(inventory.toolCount)")
                .font(.system(size: 12.5, weight: .medium))
                .foregroundColor(t.text2)
            Spacer()
            Button {
                host.refreshMcp()
            } label: {
                Text("mcp_reconnect")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(t.text2)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(t.windowBg)
                    .clipShape(RoundedRectangle(cornerRadius: 7))
                    .overlay(RoundedRectangle(cornerRadius: 7).stroke(t.border, lineWidth: 0.5))
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(dot.mix(with: t.surface, amount: 0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(dot.tint(0.24), lineWidth: 0.5))
        .padding(.bottom, 18)
    }

    private func readOnlyToolField(label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(t.text3)
            Text(value)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundStyle(t.text2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
