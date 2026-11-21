import SwiftUI

struct UnifiedInboxView: View {
    @Environment(InboxStore.self) private var inbox
    @Environment(TeamsWorkspace.self) private var teams
    let open: (InboxRow) -> Void
    let openProjects: () -> Void

    @State private var selection: Set<String> = []
    @State private var selecting = false
    @State private var options = false
    @State private var confirmingReadAll = false

    var body: some View {
        @Bindable var inbox = inbox
        List {
            filters
            if let issue = inbox.issue {
                Button(issueTitle(issue), systemImage: "exclamationmark.circle") { inbox.clearIssue(); inbox.refreshNow() }
            }
            if inbox.uid != nil && inbox.offline {
                Label("inbox_offline_saved", systemImage: "wifi.slash")
                    .foregroundStyle(.secondary)
            }
            if inbox.markingAllRead {
                ProgressView("inbox_read_batch_running")
                    .accessibilityIdentifier("inbox.read-batch-progress")
            }
            if selecting { batchMenu }
            if inbox.visible.isEmpty {
                if inbox.loading { ProgressView().accessibilityLabel("inbox_loading") }
                else { ContentUnavailableView("inbox_empty", systemImage: "tray") }
            }
            ForEach(inbox.visible) { row in
                Button { choose(row) } label: {
                    InboxConversationRow(row: row, teamName: row.category == .team
                        ? teams.teams.first(where: { $0.id == row.ref.sourceId })?.name : nil,
                        selection: selecting ? selection.contains(row.key) : nil)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("inbox.row." + row.key)
                .contextMenu { rowActions(row) }
                .swipeActions(edge: .leading, allowsFullSwipe: false) {
                    if row.isUnread || row.needsAttention {
                        Button("inbox_mark_read", systemImage: "envelope.open") { inbox.markRead([row]) }.tint(.blue)
                    } else {
                        Button("inbox_mark_unread", systemImage: "envelope.badge") { inbox.markUnread([row]) }.tint(.blue)
                    }
                }
                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                    Button(row.archived ? "inbox_unarchive" : "inbox_archive", systemImage: "archivebox") {
                        inbox.preferences([row], patch: .init(archived: !row.archived))
                    }.tint(.gray)
                }
            }
            if inbox.hasMore {
                Button("inbox_more") { inbox.loadMore() }
                    .disabled(inbox.loading)
            }
        }
        .listStyle(.plain)
        .accessibilityIdentifier("inbox.list")
        .navigationTitle("session_details_messages")
        .searchable(text: $inbox.search, prompt: "inbox_search")
        .refreshable { inbox.refreshNow() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("drawer_entry_projects", systemImage: "folder", action: openProjects)
                    .accessibilityIdentifier("inbox.open-projects")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu("inbox_options", systemImage: "ellipsis.circle") {
                    Button(selecting ? "inbox_select_done" : "inbox_select") {
                        selecting.toggle()
                        selection = []
                    }
                    Button(inbox.showArchived ? "inbox_show_inbox" : "inbox_show_archived", systemImage: "archivebox") {
                        inbox.showArchived.toggle()
                    }
                    if inbox.uid != nil {
                        Button("inbox_mark_all_read", systemImage: "envelope.open") { confirmingReadAll = true }
                            .disabled(!inbox.canMarkAllRead)
                    }
                    if inbox.uid != nil { Button("inbox_settings", systemImage: "gearshape") { options = true } }
                }
            }
        }
        .sheet(isPresented: $options) { UnifiedInboxSettings() }
        .confirmationDialog("inbox_mark_all_read_confirm", isPresented: $confirmingReadAll, titleVisibility: .visible) {
            Button("inbox_mark_all_read", action: inbox.markAllRead)
                .disabled(!inbox.canMarkAllRead)
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("inbox_mark_all_read_message")
        }
        .onChange(of: inbox.uid) { _, _ in options = false; confirmingReadAll = false; selecting = false; selection = [] }
    }

    private var filters: some View {
        @Bindable var inbox = inbox
        return VStack(alignment: .leading, spacing: 8) {
            Picker("inbox_filter", selection: $inbox.filter) {
                Text("inbox_all").tag(InboxFilter.all)
                Text("inbox_unread").tag(InboxFilter.unread)
                Text("inbox_attention").tag(InboxFilter.attention)
            }
            .pickerStyle(.menu)
            .accessibilityIdentifier("inbox.filter")
            Picker("public_extensions_category", selection: $inbox.category) {
                Text("inbox_all_categories").tag(InboxCategory?.none)
                Text("friends_title").tag(InboxCategory?.some(.friend))
                Text("teams_title").tag(InboxCategory?.some(.team))
                Text("inbox_service").tag(InboxCategory?.some(.service))
            }
            .pickerStyle(.menu)
        }
        .accessibilityElement(children: .contain)
    }

    private var batchMenu: some View {
        let selected = inbox.visible.filter { selection.contains($0.key) }
        let archived = selected.filter(\.archived)
        let unarchived = selected.filter { !$0.archived }
        return Menu("inbox_selected_actions \(selected.count)", systemImage: "checkmark.circle") {
            Button("inbox_mark_read") { inbox.markRead(selected); selection = [] }
            Button("inbox_mark_unread") { inbox.markUnread(selected); selection = [] }
            Button("inbox_mute") { inbox.preferences(selected, patch: .init(notificationMode: InboxNotificationMode.none)); selection = [] }
            if !unarchived.isEmpty {
                Button("inbox_archive") {
                    inbox.preferences(unarchived, patch: .init(archived: true))
                    selection = []
                }
            }
            if !archived.isEmpty {
                Button("inbox_unarchive") {
                    inbox.preferences(archived, patch: .init(archived: false))
                    selection = []
                }
            }
        }
        .disabled(selected.isEmpty)
    }

    @ViewBuilder
    private func rowActions(_ row: InboxRow) -> some View {
        if row.isUnread || row.needsAttention { Button("inbox_mark_read") { inbox.markRead([row]) } }
        if !row.isUnread { Button("inbox_mark_unread") { inbox.markUnread([row]) } }
        if row.pinOrder == nil {
            Button("inbox_pin") { inbox.preferences([row], patch: .init(pinOrder: InboxPush.now)) }
        } else {
            Button("inbox_unpin") { inbox.preferences([row], patch: .init(clearPin: true)) }
            Button("inbox_pin_move_up", systemImage: "arrow.up") { inbox.movePin(row, by: -1) }
                .disabled(!inbox.canMovePin(row, by: -1))
            Button("inbox_pin_move_down", systemImage: "arrow.down") { inbox.movePin(row, by: 1) }
                .disabled(!inbox.canMovePin(row, by: 1))
        }
        Button(row.notificationMode == .none ? "inbox_unmute" : "inbox_mute") {
            inbox.preferences([row], patch: .init(notificationMode: row.notificationMode == .none ? .mentions : InboxNotificationMode.none))
        }
        Button(row.archived ? "inbox_unarchive" : "inbox_archive") {
            inbox.preferences([row], patch: .init(archived: !row.archived))
        }
        if let sharing = inbox.localAssistantSharing(row) {
            Button(sharing ? "inbox_summary_keep_local" : "inbox_summary_allow_sync", systemImage: sharing ? "iphone" : "icloud") {
                inbox.setLocalAssistantSharing(!sharing, row: row)
            }
        }
    }

    private func choose(_ row: InboxRow) {
        if selecting {
            if selection.remove(row.key) == nil { selection.insert(row.key) }
        } else { open(row) }
    }

    private func issueTitle(_ issue: InboxStore.Issue) -> LocalizedStringKey {
        switch issue {
        case .storage: "inbox_storage_unavailable"
        case .backpressure: "inbox_outbox_full"
        case .sync: "inbox_sync_waiting"
        case .unavailable: "inbox_conversation_unavailable"
        case .readBatchUnavailable: "inbox_read_batch_failed"
        case .preferenceConflict: "inbox_preference_changed"
        }
    }
}

private struct InboxConversationRow: View {
    @Environment(\.dynamicTypeSize) private var dynamicType
    let row: InboxRow
    let teamName: String?
    let selection: Bool?

    var body: some View {
        let layout = dynamicType.isAccessibilitySize ? AnyLayout(VStackLayout(alignment: .leading)) : AnyLayout(HStackLayout(alignment: .top))
        layout {
            if let selection {
                Image(systemName: selection ? "checkmark.circle.fill" : "circle").accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(row.title.isEmpty ? String(localized: "inbox_untitled") : row.title)
                    .font(.headline.weight(row.isUnread ? .bold : .regular))
                context
                if !row.preview.isEmpty {
                    Text(row.preview).font(.subheadline).foregroundStyle(.secondary)
                        .lineLimit(dynamicType.isAccessibilitySize ? nil : 2)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            status
        }
        .padding(.vertical, 5)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selection == true ? .isSelected : [])
    }

    private var context: some View {
        let context = row.context ?? teamName ?? row.sourceLabel
        return Text(context.map { category + " · " + $0 } ?? category)
            .font(.caption).foregroundStyle(.secondary)
    }

    private var category: String {
        switch row.category {
        case .assistant: String(localized: "inbox_assistant")
        case .friend: String(localized: "friends_title")
        case .team: String(localized: "teams_title")
        case .service: String(localized: "inbox_service")
        }
    }

    private var status: some View {
        VStack(alignment: .leading, spacing: 5) {
            if row.pinOrder != nil { Image(systemName: "pin").accessibilityLabel("inbox_pinned") }
            if row.notificationMode == .none { Image(systemName: "bell.slash").accessibilityLabel("inbox_muted") }
            if row.offline { Label("teams_bot_avail_offline", systemImage: "wifi.slash") }
            if row.needsAttention { Label("inbox_attention", systemImage: "exclamationmark.circle") }
            if row.isUnread {
                TeamsUnreadBadge(count: min(100, max(1, row.unreadCount)))
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }
}

private struct UnifiedInboxSettings: View {
    @Environment(InboxStore.self) private var inbox
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Toggle("inbox_sync_assistant", isOn: Binding(get: { inbox.assistantSync }, set: inbox.setAssistantSync))
                } footer: {
                    Text("inbox_sync_assistant_footer")
                }
                Section {
                    Toggle("inbox_receipts", isOn: Binding(get: { inbox.dmReceipts }, set: inbox.setDMReceipts))
                } footer: {
                    Text("inbox_receipts_footer")
                }
            }
            .navigationTitle("inbox_settings")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("common_done") { dismiss() } }
            }
        }
    }
}
