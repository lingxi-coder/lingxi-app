import SwiftUI

// MARK: - MCP server list
struct MCPListPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost

    var body: some View {
        let connectedCount = store.mcpServers.filter { $0.status == .connected }.count
        let totalTools = store.mcpServers.filter { $0.enabled && $0.status == .connected }.reduce(0) { $0 + $1.tools }
        VStack(spacing: 0) {
            Text("mcp_description_blurb")
                .font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)

            HStack(spacing: 8) {
                statCard(String(localized: "mcp_stat_connected"), connectedCount)
                statCard(String(localized: "mcp_stat_available_tools"), totalTools)
            }
            .padding(.bottom, 18)

            SettingsSection(label: String(localized: "mcp_section_servers_count \(store.mcpServers.count)")) {
                ForEach(Array(store.mcpServers.enumerated()), id: \.element.id) { i, s in
                    let iconColor: Color = s.status == .connected ? Color(srgb: 0,0.78,0.55)
                        : s.status == .error ? t.statusError : t.text4
                    SettingsRow(icon: .plug, iconColor: iconColor, label: s.name,
                                subView: AnyView(HStack(spacing: 5) {
                                    Circle().fill(s.status.dot(t)).frame(width: 6, height: 6)
                                    Text("mcp_server_status_tools_detail \(s.status.label) \(s.tools) \(s.transport)")
                                        .font(.system(size: 11.5, design: .monospaced)).foregroundColor(t.text4)
                                }),
                                chevron: false, isLast: i == store.mcpServers.count - 1,
                                onTap: { host.push(.mcpEdit(s.id)) }) {
                        LXToggle(isOn: toggle(s.id))
                    }
                }
            }

            DashedAddButton(title: String(localized: "mcp_add_server"))
            (Text("mcp_transport_description_prefix")
                + Text("mcp.directory").foregroundColor(t.accent))
                .font(.system(size: 11)).foregroundColor(t.text4).lineSpacing(5)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.top, 14)
        }
    }

    private func statCard(_ label: String, _ value: Int) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label.uppercased()).font(.system(size: 10.5)).tracking(0.5).foregroundColor(t.text4)
            Text("\(value)").font(.system(size: 18, weight: .bold)).foregroundColor(t.text)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }

    private func toggle(_ id: String) -> Binding<Bool> {
        Binding(get: { store.mcpServers.first(where: { $0.id == id })?.enabled ?? false },
                set: { v in if let i = store.mcpServers.firstIndex(where: { $0.id == id }) { store.mcpServers[i].enabled = v } })
    }
}

// MARK: - MCP edit
struct MCPEditPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let mcpId: String

    private let toolNames = ["list_files", "read_file", "write_file", "create_issue", "list_issues", "comment"]

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
                Text("mcp_edit_status_tools_available \(s.status.label) \(s.tools)")
                    .font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
                Spacer()
                Button {
                    update { $0.status = .testing }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { update { $0.status = .connected } }
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
            FieldLabel(text: String(localized: "mcp_endpoint"))
            SettingsField(text: bind(\.url))
            FieldHint(String(localized: "mcp_endpoint_hint"))

            SettingsSection(label: String(localized: "mcp_section_transport_auth")) {
                SettingsRow(label: String(localized: "mcp_transport_mode"), value: s.transport, onTap: {})
                SettingsRow(label: String(localized: "mcp_auth"),
                            value: s.auth ?? String(localized: "common_none"), isLast: true, onTap: {})
            }
            SettingsSection(label: String(localized: "mcp_section_tool_permissions"),
                            footer: String(localized: "mcp_tool_permissions_footer")) {
                let tools = Array(toolNames.prefix(min(6, s.tools)))
                ForEach(Array(tools.enumerated()), id: \.element) { i, tn in
                    SettingsRow(label: tn,
                                sub: i % 2 == 0 ? String(localized: "mcp_tool_readonly") : String(localized: "mcp_tool_writable"),
                                chevron: false, isLast: i == tools.count - 1) {
                        LXToggle(isOn: .constant(i < 4))
                    }
                }
            }
            SettingsSection {
                SettingsRow(label: String(localized: "mcp_enable_server"), chevron: false) {
                    LXToggle(isOn: Binding(get: { s.enabled }, set: { v in update { $0.enabled = v } }))
                }
                SettingsRow(label: String(localized: "mcp_auto_start"), chevron: false, isLast: true) { LXToggle(isOn: .constant(true)) }
            }
            Button { store.mcpServers.removeAll { $0.id == mcpId }; host.pop() } label: {
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
    private func update(_ mutate: (inout MCPServer) -> Void) {
        if let i = store.mcpServers.firstIndex(where: { $0.id == mcpId }) { mutate(&store.mcpServers[i]) }
    }
}
