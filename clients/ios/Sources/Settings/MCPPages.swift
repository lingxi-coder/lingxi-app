import SwiftUI

// MARK: - MCP server list
struct MCPListPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost

    var body: some View {
        let connectedCount = store.mcpServers.filter { $0.status == .connected }.count
        let connectedServers = store.mcpServers
            .filter { $0.enabled && $0.status == .connected }
        let knownTools = connectedServers.compactMap(\.tools).reduce(0, +)
        let toolCount = connectedServers.allSatisfy { $0.tools != nil } ? "\(knownTools)" : "—"
        VStack(spacing: 0) {
            Text("mcp_description_blurb")
                .font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)

            HStack(spacing: 8) {
                statCard(String(localized: "mcp_stat_connected"), "\(connectedCount)")
                statCard(String(localized: "mcp_stat_available_tools"), toolCount)
            }
            .padding(.bottom, 18)

            SettingsSection(label: String(localized: "mcp_section_servers_count \(store.mcpServers.count)")) {
                ForEach(Array(store.mcpServers.enumerated()), id: \.element.id) { i, s in
                    let iconColor: Color = s.status == .connected ? Color(srgb: 0,0.78,0.55)
                        : s.status == .error ? t.statusError : t.text4
                    SettingsRow(icon: .plug, iconColor: iconColor, label: s.name,
                                subView: AnyView(VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 5) {
                                        Circle().fill(s.status.dot(t)).frame(width: 6, height: 6)
                                        Text("mcp_server_status_tools_detail \(s.status.label) \(s.tools.map(String.init) ?? "—") \(s.transportLabel)")
                                            .font(.system(size: 11.5, design: .monospaced)).foregroundColor(t.text4)
                                    }
                                    Text(s.endpointSummary)
                                        .font(.system(size: 10.5, design: .monospaced))
                                        .foregroundColor(t.text4)
                                        .lineLimit(1)
                                }),
                                chevron: false, isLast: i == store.mcpServers.count - 1,
                                onTap: { host.push(.mcpEdit(s.id)) }) {
                        LXToggle(isOn: toggle(s.id))
                    }
                }
            }

            if store.mcpServers.isEmpty {
                Text(store.mcpListingLoaded
                    ? String(localized: "mcp_empty_state")
                    : String(localized: "mcp_loading_state"))
                    .font(.system(size: 12))
                    .foregroundColor(t.text4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.bottom, 12)
            }

            DashedAddButton(title: String(localized: "mcp_add_server"), action: host.addMcpServer)
            (Text("mcp_transport_description_prefix")
                + Text("mcp.directory").foregroundColor(t.accent))
                .font(.system(size: 11)).foregroundColor(t.text4).lineSpacing(5)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 14)
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

    private func toggle(_ id: String) -> Binding<Bool> {
        Binding(get: { store.mcpServers.first(where: { $0.id == id })?.enabled ?? false },
                set: { v in
                    guard let i = store.mcpServers.firstIndex(where: { $0.id == id }) else { return }
                    let previous = store.mcpServers[i].enabled
                    store.mcpServers[i].enabled = v
                    if !host.saveMcpServer(store.mcpServers[i]) {
                        store.mcpServers[i].enabled = previous
                    }
                })
    }
}

// MARK: - MCP edit
struct MCPEditPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let mcpId: String

    var body: some View {
        guard let s = store.mcpServers.first(where: { $0.id == mcpId }) else {
            DispatchQueue.main.async { host.pop() }
            return AnyView(EmptyView())
        }
        let dot = s.status.dot(t)
        return AnyView(VStack(spacing: 0) {
            HStack(spacing: 8) {
                Circle().fill(dot).frame(width: 8, height: 8)
                    .overlay(Circle().stroke(dot.tint(0.22), lineWidth: 3).scaleEffect(1.75))
                Text("mcp_edit_status_tools_available \(s.status.label) \(s.tools.map(String.init) ?? "—")")
                    .font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
                Spacer()
                Button {
                    host.refreshMcp()
                } label: {
                    Text("mcp_reconnect").font(.system(size: 12, weight: .medium)).foregroundColor(t.text2)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 7))
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
                if s.transport == "stdio" {
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
                SettingsRow(label: String(localized: "mcp_auth"),
                            value: s.auth ?? String(localized: "common_none"), isLast: true, onTap: {})
            }
            SettingsSection(label: String(localized: "mcp_section_tool_permissions"),
                            footer: String(localized: "mcp_tool_permissions_footer")) {
                if let tools = s.tools {
                    SettingsRow(label: String(localized: "mcp_tools_discovered_count \(tools)"),
                                chevron: false, isLast: true) {
                        LXIcon(name: .check, size: 15, color: t.ok, stroke: 2.2)
                    }
                } else {
                    SettingsRow(label: String(localized: "mcp_tools_unavailable"),
                                sub: String(localized: "mcp_tools_unavailable_sub"),
                                chevron: false, isLast: true)
                }
            }
            SettingsSection {
                SettingsRow(label: String(localized: "mcp_enable_server"), chevron: false) {
                    LXToggle(isOn: Binding(get: { s.enabled }, set: { v in update { $0.enabled = v } }))
                }
            }
            Button { host.removeMcpServer(s) } label: {
                Text("mcp_remove_server").font(.system(size: 13.5, weight: .medium)).foregroundColor(t.danger)
                    .frame(maxWidth: .infinity).padding(12)
                    .overlay(RoundedRectangle(cornerRadius: 11).stroke(t.border, lineWidth: 0.5))
            }
            .padding(.top, 8)
        })
    }

    private func bind(_ kp: WritableKeyPath<MCPServer, String>) -> Binding<String> {
        Binding(get: { store.mcpServers.first(where: { $0.id == mcpId })?[keyPath: kp] ?? "" },
                set: { v in update { $0[keyPath: kp] = v } })
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
        if let i = store.mcpServers.firstIndex(where: { $0.id == mcpId }) { mutate(&store.mcpServers[i]) }
    }
}
