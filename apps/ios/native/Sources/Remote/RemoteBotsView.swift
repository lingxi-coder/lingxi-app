import SwiftUI

#if canImport(harness_runtimeFFI)
/// A session in a list: a computer's shared sessions and a Bot's chats look alike.
struct RemoteSessionRow: View {
    let session: RemoteSessionSummaryFfi
    /// A computer's sessions keep their project line even when it is empty, as they always did; a Bot's
    /// chats mostly have no project, and do without the line.
    var hidesEmptyProject = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(session.title.isEmpty ? String(localized: "remote_untitled") : session.title)
            if !(hidesEmptyProject && session.projectLabel.isEmpty) {
                Text(session.projectLabel).font(.caption).foregroundStyle(.secondary)
            }
            Text(status).font(.caption).foregroundStyle(.secondary)
        }
    }

    private var status: LocalizedStringKey {
        switch session.status {
        case .running: "remote_status_running"
        case .requiresAction: "remote_status_requires_action"
        case .idle: "remote_status_idle"
        case .dormant: "remote_status_dormant"
        }
    }
}

#endif
