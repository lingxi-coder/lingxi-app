import SwiftUI

/// A Bot's shared list (Desktop `SharedListDialog`): the people who may use the Bot and the Bot's own runs change the same items.
/// A change is saved only on top of the list it was made from. When someone else, or the Bot, saved first, nothing is saved:
/// the list is read again, what the person typed stays where it is, and saving it again is their decision.
struct TeamSharedListSheet: View {
    private struct Edit: Equatable { let itemId: String; var text: String }

    let bot: TeamBotCard
    let model: TeamMessagingModel
    let onClose: () -> Void
    @State private var list: TeamSharedList?
    @State private var text = ""
    @State private var editing: Edit?
    @State private var busy = false
    @State private var error = ""
    @State private var moved = false
    @State private var operation: TeamSharedListOperation?

    var body: some View {
        NavigationStack {
            List {
                if let list {
                    Section {
                        TeamsNote(text: "teams_list_hint \(bot.name)", systemImage: "info.circle")
                        if moved {
                            Label("teams_list_moved", systemImage: "exclamationmark.triangle")
                                .font(.footnote).foregroundStyle(.orange)
                                .accessibilityIdentifier("teams.list.moved")
                        }
                    }
                    Section {
                        ForEach(list.items) { item in row(item) }
                        if list.items.isEmpty { Text("teams_list_empty").font(.footnote).foregroundStyle(.secondary) }
                    }
                    Section {
                        HStack(spacing: 10) {
                            TextField("teams_list_add_placeholder", text: $text)
                                .submitLabel(.done).onSubmit(add)
                                .accessibilityIdentifier("teams.list.new")
                            Button("teams_list_add", action: add)
                                .buttonStyle(.borderedProminent).controlSize(.small)
                                .disabled(busy || !TeamSharedListRules.usable(text))
                                .accessibilityIdentifier("teams.list.add")
                        }
                        if !TeamSharedListRules.fits(text) { Text("teams_list_too_long").font(.footnote).foregroundStyle(.red) }
                    }
                } else if error.isEmpty {
                    HStack(spacing: 10) { ProgressView(); Text("teams_list_loading").foregroundStyle(.secondary) }
                }
                if !error.isEmpty {
                    Section {
                        Text(LocalizedStringKey(TeamsErrors.key(error))).foregroundStyle(.red).accessibilityIdentifier("teams.list.error")
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle(list?.title ?? bot.sharedList?.title ?? "")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_close", action: onClose) }
                ToolbarItem(placement: .primaryAction) {
                    Button("teams_list_refresh", systemImage: "arrow.clockwise") { attempt { moved = false; try await read() } }
                        .disabled(busy)
                        .accessibilityIdentifier("teams.list.refresh")
                }
            }
        }
        .task { attempt { try await read() } }
        .accessibilityIdentifier("teams.list.\(bot.sharedBotId)")
    }

    @ViewBuilder
    private func row(_ item: TeamSharedListItem) -> some View {
        if let edit = editing, edit.itemId == item.itemId {
            let next = edit.text.trimmingCharacters(in: .whitespacesAndNewlines)
            VStack(alignment: .leading, spacing: 8) {
                TextField("teams_list_edit_title", text: Binding(get: { edit.text }, set: { editing = Edit(itemId: item.itemId, text: $0) }), axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("teams.list.edit.\(item.itemId)")
                if !TeamSharedListRules.fits(edit.text) { Text("teams_list_too_long").font(.footnote).foregroundStyle(.red) }
                HStack(spacing: 10) {
                    Button("common_save") { write([.setText(itemId: item.itemId, text: next)]) { editing = nil } }
                        .buttonStyle(.borderedProminent)
                        .disabled(busy || !TeamSharedListRules.usable(edit.text) || next == item.text)
                    Button("common_cancel") { editing = nil }.buttonStyle(.bordered)
                }
                .controlSize(.small)
            }
        } else {
            HStack(spacing: 10) {
                Button { write([.setDone(itemId: item.itemId, done: !item.done)]) } label: {
                    Label {
                        Text(item.text).strikethrough(item.done).foregroundStyle(item.done ? Color.secondary : Color.primary)
                            .multilineTextAlignment(.leading).frame(maxWidth: .infinity, alignment: .leading)
                    } icon: {
                        Image(systemName: item.done ? "checkmark.circle.fill" : "circle").foregroundStyle(item.done ? Color.accentColor : Color.secondary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityValue(Text(item.done ? "teams_list_done" : "teams_list_not_done"))
                .accessibilityIdentifier("teams.list.item.\(item.itemId)")
                Menu {
                    Button("teams_list_edit", systemImage: "pencil") { editing = Edit(itemId: item.itemId, text: item.text) }
                    Button("teams_list_remove", systemImage: "trash", role: .destructive) { write([.remove(itemId: item.itemId)]) }
                } label: {
                    Image(systemName: "ellipsis").frame(width: 44, height: 32).contentShape(Rectangle())
                }
                .accessibilityLabel(Text("teams_list_item_options \(item.text)"))
            }
            .disabled(busy)
        }
    }

    private func add() {
        guard TeamSharedListRules.usable(text) else { return }
        write([.add(text: text.trimmingCharacters(in: .whitespacesAndNewlines))]) { text = "" }
    }

    /// One request at a time; a failure is shown under the list and never clears what was typed.
    private func attempt(_ work: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }
        busy = true; error = ""
        Task { @MainActor in
            do { try await work() } catch let failure as TeamMessageFailure { error = failure.code } catch { error = "SERVICE_UNAVAILABLE" }
            busy = false
        }
    }

    private func read() async throws {
        let next = try await model.readList(bot.sharedBotId)
        list = next
        if let editing, !next.items.contains(where: { $0.itemId == editing.itemId }) { self.editing = nil }
    }

    private func write(_ changes: [TeamSharedListChange], saved: @escaping @MainActor () -> Void = {}) {
        guard let list else { return }
        attempt {
            let current = TeamSharedListOperation.of(operation, revision: list.dataRevision, changes: changes)
            operation = current; moved = false
            do {
                try await model.writeList(bot.sharedBotId, operationId: current.id, expectedDataRevision: list.dataRevision, changes: changes)
            } catch let failure as TeamMessageFailure where TeamSharedListRules.moved(failure.code) {
                try await read(); moved = true; return
            }
            saved(); try await read()
        }
    }
}
