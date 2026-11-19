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
                Label("Offline · Saved conversations", systemImage: "wifi.slash")
                    .foregroundStyle(.secondary)
            }
            if inbox.markingAllRead {
                ProgressView("Marking conversations as read")
                    .accessibilityIdentifier("inbox.read-batch-progress")
            }
            if selecting { batchMenu }
            if inbox.visible.isEmpty {
                if inbox.loading { ProgressView().accessibilityLabel("Loading conversations") }
                else { ContentUnavailableView("No conversations", systemImage: "tray") }
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
                        Button("Mark as read", systemImage: "envelope.open") { inbox.markRead([row]) }.tint(.blue)
                    } else {
                        Button("Mark as unread", systemImage: "envelope.badge") { inbox.markUnread([row]) }.tint(.blue)
                    }
                }
                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                    Button(row.archived ? "Unarchive" : "Archive", systemImage: "archivebox") {
                        inbox.preferences([row], patch: .init(archived: !row.archived))
                    }.tint(.gray)
                }
            }
            if inbox.hasMore {
                Button("Load more conversations") { inbox.loadMore() }
                    .disabled(inbox.loading)
            }
        }
        .listStyle(.plain)
        .accessibilityIdentifier("inbox.list")
        .navigationTitle("session_details_messages")
        .searchable(text: $inbox.search, prompt: "Search by title")
        .refreshable { inbox.refreshNow() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("drawer_entry_projects", systemImage: "folder", action: openProjects)
                    .accessibilityIdentifier("inbox.open-projects")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu("Conversation options", systemImage: "ellipsis.circle") {
                    Button(selecting ? "Done selecting" : "Select conversations") {
                        selecting.toggle()
                        selection = []
                    }
                    Button(inbox.showArchived ? "Show inbox" : "Show archived", systemImage: "archivebox") {
                        inbox.showArchived.toggle()
                    }
                    if inbox.uid != nil {
                        Button("Mark all conversations as read", systemImage: "envelope.open") { confirmingReadAll = true }
                            .disabled(!inbox.canMarkAllRead)
                    }
                    if inbox.uid != nil { Button("Inbox settings", systemImage: "gearshape") { options = true } }
                }
            }
        }
        .sheet(isPresented: $options) { UnifiedInboxSettings() }
        .confirmationDialog("Mark all conversations as read?", isPresented: $confirmingReadAll, titleVisibility: .visible) {
            Button("Mark all as read", action: inbox.markAllRead)
                .disabled(!inbox.canMarkAllRead)
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Marks unread conversations in your inbox, including ones you haven’t loaded. Pending requests remain open.")
        }
        .onChange(of: inbox.uid) { _, _ in options = false; confirmingReadAll = false; selecting = false; selection = [] }
    }

    private var filters: some View {
        @Bindable var inbox = inbox
        return VStack(alignment: .leading, spacing: 8) {
            Picker("Inbox filter", selection: $inbox.filter) {
                Text("All").tag(InboxFilter.all)
                Text("Unread").tag(InboxFilter.unread)
                Text("Needs attention").tag(InboxFilter.attention)
            }
            .pickerStyle(.menu)
            .accessibilityIdentifier("inbox.filter")
            Picker("Category", selection: $inbox.category) {
                Text("All categories").tag(InboxCategory?.none)
                Text("Friends").tag(InboxCategory?.some(.friend))
                Text("Teams").tag(InboxCategory?.some(.team))
                Text("Services").tag(InboxCategory?.some(.service))
            }
            .pickerStyle(.menu)
        }
        .accessibilityElement(children: .contain)
    }

    private var batchMenu: some View {
        let selected = inbox.visible.filter { selection.contains($0.key) }
        let archived = selected.filter(\.archived)
        let unarchived = selected.filter { !$0.archived }
        return Menu("Actions for \(selected.count) selected", systemImage: "checkmark.circle") {
            Button("Mark as read") { inbox.markRead(selected); selection = [] }
            Button("Mark as unread") { inbox.markUnread(selected); selection = [] }
            Button("Mute") { inbox.preferences(selected, patch: .init(notificationMode: InboxNotificationMode.none)); selection = [] }
            if !unarchived.isEmpty {
                Button("Archive") {
                    inbox.preferences(unarchived, patch: .init(archived: true))
                    selection = []
                }
            }
            if !archived.isEmpty {
                Button("Unarchive") {
                    inbox.preferences(archived, patch: .init(archived: false))
                    selection = []
                }
            }
        }
        .disabled(selected.isEmpty)
    }

    @ViewBuilder
    private func rowActions(_ row: InboxRow) -> some View {
        if row.isUnread || row.needsAttention { Button("Mark as read") { inbox.markRead([row]) } }
        if !row.isUnread { Button("Mark as unread") { inbox.markUnread([row]) } }
        if row.pinOrder == nil {
            Button("Pin") { inbox.preferences([row], patch: .init(pinOrder: InboxPush.now)) }
        } else {
            Button("Unpin") { inbox.preferences([row], patch: .init(clearPin: true)) }
            Button("Move pin up", systemImage: "arrow.up") { inbox.movePin(row, by: -1) }
                .disabled(!inbox.canMovePin(row, by: -1))
            Button("Move pin down", systemImage: "arrow.down") { inbox.movePin(row, by: 1) }
                .disabled(!inbox.canMovePin(row, by: 1))
        }
        Button(row.notificationMode == .none ? "Unmute" : "Mute") {
            inbox.preferences([row], patch: .init(notificationMode: row.notificationMode == .none ? .mentions : InboxNotificationMode.none))
        }
        Button(row.archived ? "Unarchive" : "Archive") {
            inbox.preferences([row], patch: .init(archived: !row.archived))
        }
        if let sharing = inbox.localAssistantSharing(row) {
            Button(sharing ? "Keep summary on this device" : "Allow summary sync", systemImage: sharing ? "iphone" : "icloud") {
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
        case .storage: "Inbox storage unavailable"
        case .backpressure: "Sync queue is full. Try again after syncing."
        case .sync: "Changes are waiting to sync"
        case .unavailable: "A conversation is no longer available"
        case .readBatchUnavailable: "The server couldn’t finish marking all conversations as read. Try again when connected."
        case .preferenceConflict: "Conversation settings changed on another device. Review the updated settings and try again."
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
                Text(row.title.isEmpty ? String(localized: "Conversation") : row.title)
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
        case .assistant: String(localized: "Assistant")
        case .friend: String(localized: "Friends")
        case .team: String(localized: "Teams")
        case .service: String(localized: "Services")
        }
    }

    private var status: some View {
        VStack(alignment: .leading, spacing: 5) {
            if row.pinOrder != nil { Image(systemName: "pin").accessibilityLabel("Pinned") }
            if row.notificationMode == .none { Image(systemName: "bell.slash").accessibilityLabel("Muted") }
            if row.offline { Label("Host offline", systemImage: "wifi.slash") }
            if row.needsAttention { Label("Needs attention", systemImage: "exclamationmark.circle") }
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
                    Toggle("Sync assistant summaries", isOn: Binding(get: { inbox.assistantSync }, set: inbox.setAssistantSync))
                } footer: {
                    Text("Sync activity and read status with this account. Opening the conversation still requires access to its source device.")
                }
                Section {
                    Toggle("Share read receipts in direct messages", isOn: Binding(get: { inbox.dmReceipts }, set: inbox.setDMReceipts))
                } footer: {
                    Text("Receipts are visible only when both people enable them.")
                }
            }
            .navigationTitle("Inbox settings")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
    }
}
