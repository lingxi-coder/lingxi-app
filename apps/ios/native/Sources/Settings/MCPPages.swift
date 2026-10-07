import SwiftUI

private func localized(_ key: String) -> String {
    String(localized: String.LocalizationValue(stringLiteral: key))
}

private func digestSummary(_ value: String) -> String {
    guard value.count > 12 else { return value }
    return String(value.prefix(12))
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
        if let index = store.mcpServers.firstIndex(where: { $0.id == mcpId }) {
            mutate(&store.mcpServers[index])
        }
    }
}

