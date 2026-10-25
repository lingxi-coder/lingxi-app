import SwiftUI

struct PermissionModePage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    @State private var pending: String?

    private var options: [RadioOption] { [
        RadioOption(value: "default", label: String(localized: "settings_permission_mode_default"), sub: String(localized: "settings_permission_mode_default_detail")),
        RadioOption(value: "acceptEdits", label: String(localized: "settings_permission_mode_accept_edits"), sub: String(localized: "settings_permission_mode_accept_edits_detail")),
        RadioOption(value: "plan", label: String(localized: "settings_permission_mode_plan"), sub: String(localized: "settings_permission_mode_plan_detail")),
        RadioOption(value: "auto", label: String(localized: "settings_permission_mode_auto"), sub: String(localized: "settings_permission_mode_auto_detail")),
        RadioOption(value: "dontAsk", label: String(localized: "settings_permission_mode_dont_ask"), sub: String(localized: "settings_permission_mode_dont_ask_detail")),
    ] }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSection(label: String(localized: "settings_permission_mode_title"), footer: String(localized: "settings_permission_mode_session_footer")) {
                RadioList(options: options, value: Binding(
                    get: { store.permissionMode },
                    set: { mode in
                        if mode == "dontAsk" || mode == "bypassPermissions" {
                            pending = mode
                        } else {
                            host.applyPermissionMode(mode)
                        }
                    }
                ))
            }
            if store.effectivePermissionMode != store.permissionMode {
                Text(String(localized: "settings_permission_mode_effective \(store.effectivePermissionMode)"))
                    .font(.caption).foregroundColor(.secondary).padding(.horizontal, 4)
            }
            if let error = store.permissionModeError, !error.isEmpty {
                Text(error)
                    .font(.caption).foregroundColor(.red).padding(.horizontal, 4)
            }
        }
        .alert(String(localized: "settings_permission_mode_confirm_title"), isPresented: Binding(
            get: { pending != nil },
            set: { if !$0 { pending = nil } }
        )) {
            Button(String(localized: "settings_permission_mode_confirm"), role: .destructive) {
                if let mode = pending { host.applyPermissionMode(mode) }
                pending = nil
            }
            Button(String(localized: "common_cancel"), role: .cancel) { pending = nil }
        } message: {
            Text(String(localized: "settings_permission_mode_confirm_detail"))
        }
    }
}

struct TypeScriptLspModePage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost

    private var options: [RadioOption] { [
        RadioOption(value: "auto", label: String(localized: "settings_typescript_lsp_auto"), sub: String(localized: "settings_typescript_lsp_auto_detail")),
        RadioOption(value: "off", label: String(localized: "settings_typescript_lsp_off"), sub: String(localized: "settings_typescript_lsp_off_detail")),
        RadioOption(value: "on", label: String(localized: "settings_typescript_lsp_on"), sub: String(localized: "settings_typescript_lsp_on_detail")),
    ] }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSection(
                label: String(localized: "settings_typescript_lsp_title"),
                footer: String(localized: "settings_typescript_lsp_pinned_footer")
            ) {
                RadioList(
                    options: options,
                    value: Binding(
                        get: { store.typescriptLspMode },
                        set: { host.applyTypescriptLspMode($0) }
                    )
                )
            }
            if !store.typescriptLspAvailable && store.typescriptLspMode != "off" {
                Text(String(localized: "settings_typescript_lsp_unavailable"))
                    .font(.caption).foregroundColor(.secondary).padding(.horizontal, 4)
            } else if store.effectiveTypescriptLspMode != store.typescriptLspMode {
                Text(String(localized: "settings_typescript_lsp_effective \(store.effectiveTypescriptLspMode)"))
                    .font(.caption).foregroundColor(.secondary).padding(.horizontal, 4)
            }
            if let error = store.typescriptLspError, !error.isEmpty {
                Text(error)
                    .font(.caption).foregroundColor(.red).padding(.horizontal, 4)
            }
        }
    }
}
