import SwiftUI

/// The focused home for a running Local App. It deliberately keeps the host
/// chrome to a few recoverable actions so the app's own UI remains the work.
struct LocalAppWorkspaceView: View {
    @Bindable var store: LocalAppsStore
    let appID: String
    let activeConversationID: String
    let onShowSessions: () -> Void
    let onDeleted: (String) -> Void

    @State private var showsManagementSheet = false

    private var app: LocalAppSummary? { store.app(id: appID) }
    private var runtime: LocalAppRuntimeStatus { store.runtimes[appID] ?? .stopped }
    private var runtimeURL: URL? { runtime.url }

    var body: some View {
        Group {
            if let runtimeURL {
                LocalAppWebView(
                    appID: appID,
                    url: runtimeURL,
                    onBridgeRequest: { request in
                        Task { await store.executeBridge(request) }
                    }
                )
                .ignoresSafeArea(.container, edges: [.horizontal, .bottom])
            } else {
                placeholder
            }
        }
        .navigationTitle(app?.displayName ?? String(localized: "local_apps_title"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button(action: onShowSessions) {
                    Label("local_apps_section_sessions", systemImage: "sidebar.left")
                        .labelStyle(.iconOnly)
                        .frame(width: 44, height: 44)
                }
                .accessibilityIdentifier("local-apps.workspace.sessions")
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                if case .running = runtime {
                    Button {
                        Task { await store.stop(appID: appID) }
                    } label: {
                        Label("local_apps_run_pause", systemImage: "pause.fill")
                            .labelStyle(.iconOnly)
                            .frame(width: 44, height: 44)
                    }
                    .accessibilityIdentifier("local-apps.workspace.stop")
                }
                Button { showsManagementSheet = true } label: {
                    Label("local_apps_more", systemImage: "ellipsis.circle")
                        .labelStyle(.iconOnly)
                        .frame(width: 44, height: 44)
                }
                .accessibilityIdentifier("local-apps.workspace.manage")
            }
        }
        .sheet(isPresented: $showsManagementSheet) {
            LocalAppManagementSheet(
                store: store,
                appID: appID,
                activeConversationID: activeConversationID,
                onDeleted: onDeleted
            )
        }
        .task(id: appID) {
            await store.getDetails(appID: appID)
        }
    }

    @ViewBuilder
    private var placeholder: some View {
        switch LocalAppPreviewPlaceholder.forStatus(store.runtimes[appID]) {
        case .transient, .none:
            ContentUnavailableView {
                Label("local_apps_preview_not_ready", systemImage: "hourglass")
            } description: {
                ProgressView(runtime.label)
            }
        case let .failed(reason):
            ContentUnavailableView {
                Label("local_apps_runtime_failed_title", systemImage: "exclamationmark.triangle")
            } description: {
                Text(reason)
            } actions: {
                Button("common_retry") { Task { await store.start(appID: appID) } }
                    .buttonStyle(.borderedProminent)
            }
        case let .suspended(reason):
            ContentUnavailableView {
                Label("local_apps_runtime_suspended_plain", systemImage: "pause.circle")
            } description: {
                if let reason { Text(reason) }
            } actions: {
                startButton
            }
        case .idle:
            ContentUnavailableView {
                Label("local_apps_preview_not_running", systemImage: localAppIconSystemName)
            } description: {
                Text("local_apps_preview_not_running_detail")
            } actions: {
                startButton
            }
        }
    }

    private var startButton: some View {
        Button("common_start") { Task { await store.start(appID: appID) } }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("local-apps.workspace.start")
    }
}
