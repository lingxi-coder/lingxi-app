import SwiftUI

/// What a task did outside the Chat (Desktop `OperationsDialog`). A task that needs checking ends only when each action nobody
/// could verify is known to have happened or not; the Bot's maintainer says which, after checking where the action took place.
/// The requester sees the same list without the choice.
struct TeamOperationsSheet: View {
    @Environment(\.theme) private var theme
    let taskId: String
    let model: TeamMessagingModel?
    let canResolve: Bool
    let members: [TeamMember]
    let onClose: () -> Void
    @State private var rows: [TeamTaskOperation]?
    @State private var busy = false
    @State private var error = ""

    var body: some View {
        NavigationStack {
            List {
                if let rows {
                    if rows.contains(where: \.unverified) {
                        Section { Text(canResolve ? "teams_ops_intro_maintainer" : "teams_ops_intro_requester").font(.footnote).foregroundStyle(.secondary) }
                    }
                    if rows.isEmpty { Text("teams_ops_empty").foregroundStyle(.secondary) }
                    ForEach(rows) { row in
                        Section { operation(row) }
                    }
                } else if error.isEmpty {
                    HStack(spacing: 8) { ProgressView(); Text("teams_doc_loading").foregroundStyle(.secondary) }
                }
                if !error.isEmpty {
                    Section {
                        Text(LocalizedStringKey(TeamOperationRules.errorKey(error))).font(.footnote).foregroundStyle(.red)
                            .accessibilityIdentifier("teams.ops.error")
                    }
                }
            }
            .navigationTitle("teams_ops_title")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("common_close", action: onClose) }
                ToolbarItem(placement: .cancellationAction) {
                    Button("teams_list_refresh", systemImage: "arrow.clockwise") { attempt(load) }.disabled(busy)
                }
            }
        }
        .task { attempt(load) }
        .accessibilityIdentifier("teams.ops.\(taskId)")
    }

    @ViewBuilder
    private func operation(_ row: TeamTaskOperation) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(verbatim: row.tool.name).font(.body.weight(.semibold)).foregroundStyle(theme.text)
            Text(LocalizedStringKey(TeamOperationRules.stateKey(row))).font(.subheadline)
                .foregroundStyle(row.unverified ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
            Text(verbatim: detail(row)).font(.caption).foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("teams.ops.row.\(row.operationId)")
        if canResolve && row.unverified {
            Button("teams_ops_happened") { resolve(row, happened: true) }
                .accessibilityLabel(Text("teams_ops_happened_label \(row.tool.name)"))
                .accessibilityIdentifier("teams.ops.happened.\(row.operationId)")
                .disabled(busy)
            Button("teams_ops_not_happened") { resolve(row, happened: false) }
                .accessibilityLabel(Text("teams_ops_not_happened_label \(row.tool.name)"))
                .accessibilityIdentifier("teams.ops.not_happened.\(row.operationId)")
                .disabled(busy)
        }
    }

    private func detail(_ row: TeamTaskOperation) -> String {
        let when = Date(timeIntervalSince1970: row.dispatchedAt / 1000).formatted(date: .abbreviated, time: .shortened)
        guard let by = row.resolvedBy else { return when }
        let name = members.first { $0.uid == by.uid }?.displayName ?? ""
        return "\(when) · " + String(localized: "teams_ops_confirmed_by \(name.isEmpty ? TeamsListStrings.current.member : name)")
    }

    private func service() throws -> TeamMessagingModel {
        guard let model else { throw TeamMessageFailure(code: "INVALID_SESSION") }
        return model
    }

    @MainActor private func load() async throws { rows = try await service().listOperations(taskId) }

    /// One request at a time. After a decision the list is read again whether or not it was accepted, so it never shows a choice that is gone.
    private func attempt(_ work: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }
        busy = true; error = ""
        Task { @MainActor in
            do { try await work() } catch let failure as TeamMessageFailure { error = failure.code } catch { error = "SERVICE_UNAVAILABLE" }
            busy = false
        }
    }

    private func resolve(_ row: TeamTaskOperation, happened: Bool) {
        attempt {
            do { try await service().resolveOperation(taskId, operationId: row.operationId, happened: happened) }
            catch { try? await load(); throw error }
            try await load()
        }
    }
}
