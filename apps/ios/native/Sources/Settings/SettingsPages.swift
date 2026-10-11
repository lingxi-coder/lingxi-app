import SwiftUI

// MARK: - Page dispatcher
struct SettingsPages: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let page: SettingsPage

    var body: some View {
        switch page {
        case .main:                       MainSettingsPage(store: store, host: host)
        case .account:                    DesktopAccountPage(host: host)
        case .providerList(let b):
            VStack {
                ProviderListPage(store: store, host: host, kind: b.kind)
                if b.kind == .llm { EngineProfileCredentialsEditor() }
            }
        case .providerPicker(let b):      ProviderPickerPage(store: store, host: host, kind: b.kind)
        case .providerEdit(let b, let id): ProviderEditPage(store: store, host: host, kind: b.kind, providerId: id)
        case .voice:                      VoicePage(store: store)
        case .linuxRuntime:               LinuxRuntimePage(store: store, onOpenTerminal: host.openTerminal)
        case .knowledge:                  KnowledgePage()
        case .memory:                     MemoryPage()
        case .workflows:                  WorkflowsPage()
        case .appearance:                 AppearancePage()
        case .language:                   LanguagePage()
        case .notifications:              NotificationsPage(store: store)
        case .input:                      InputPage()
        case .appIntegration:             AppIntegrationPage()
        case .privacy:                    PrivacyPage()
        case .permissionMode:             PermissionModePage(store: store, host: host)
        case .typescriptLsp:              TypeScriptLspModePage(store: store, host: host)
        case .skills:                     DesktopSkillsAdminPage()
        case .skillDetail(let id):        SkillDetailPage(store: store, host: host, skillId: id)
        case .mcpList:                    DesktopMCPAdminPage(host: host)
        case .mcpEdit(let id):            MCPEditPage(store: store, host: host, mcpId: id)
        case .dream:                      DreamPage(store: store)
        case .general:                    DesktopGeneralPage(store: store, host: host)
        case .customProviders:            DesktopCustomProvidersPage()
        case .fusion:
            ContentUnavailableView("settings_parity_fusion", systemImage: "sparkles", description: Text("settings_parity_fusion_unavailable"))
                .accessibilityIdentifier("settings.fusion.unsupported")
        case .permissions:                DesktopPermissionsPage(host: host)
        case .toolsAgent:                 DesktopToolsAgentPage()
        case .hooks:                      DesktopHooksPage()
        case .plugins:                    DesktopPluginsPage(host: host)
        case .diagnostics:                DesktopDiagnosticsPage()
        case .about:                      DesktopAboutPage()
        case .archivedChats:              DesktopArchivedChatsPage(projectStore: host.projectStore)
        case .projectsTrust:              DesktopProjectsPage(projectStore: host.projectStore)
        }
    }
}
