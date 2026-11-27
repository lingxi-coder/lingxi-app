import SwiftUI

struct TeamVisualizationSettings: View {
    let conversation: String
    let model: TeamMessagingModel
    @State private var settings: Configuration?
    @State private var limit = ""
    @State private var saving = false
    @State private var failed = false
    private var ceiling: Int { TeamVisualizationTransfer.maximumBytes / (1024 * 1024) }
    private var proposedLimit: Int? {
        guard let value = Int(limit), (1...ceiling).contains(value) else { return nil }
        return value
    }

    private struct Configuration {
        let limit: Int
        let revision: Int
        let canEdit: Bool

        init(_ value: [String: Any]) throws {
            guard let limit = value["limitMiB"] as? Int, let revision = value["revision"] as? Int,
                  let canEdit = value["canEdit"] as? Bool else { throw TeamMessageFailure(code: "INVALID_DATA") }
            self.limit = limit; self.revision = revision; self.canEdit = canEdit
        }
    }

    var body: some View {
        Section("teams_visualization_limit") {
            if let settings {
                if settings.canEdit {
                    HStack {
                        TextField("teams_visualization_limit", text: $limit)
                            .keyboardType(.numberPad).disabled(saving)
                        Text("MiB")
                    }
                    Button("common_save") { Task { await save(settings) } }
                        .disabled(saving || proposedLimit == nil || proposedLimit == settings.limit)
                } else {
                    Text("\(settings.limit) MiB")
                }
            }
            if failed { Text("teams_data_unavailable").foregroundStyle(.secondary) }
        }
        .task(id: conversation) { await reload() }
    }

    private func reload() async {
        do {
            let response = try await model.visualization(conversationId: conversation, body: ["action": "settings"])
            let current = try Configuration(response)
            guard (1...ceiling).contains(current.limit) else { throw TeamMessageFailure(code: "INVALID_DATA") }
            settings = current
            limit = String(current.limit)
        } catch is CancellationError { }
        catch { failed = true }
    }

    private func save(_ configuration: Configuration) async {
        guard !saving, configuration.canEdit, let limit = proposedLimit, limit != configuration.limit else { return }
        saving = true
        failed = false
        defer { saving = false }
        do {
            let response = try await model.visualization(conversationId: conversation, body: [
                "action": "setLimit", "limitMiB": limit, "expectedRevision": configuration.revision,
            ])
            settings = try Configuration(response)
        } catch is CancellationError { }
        catch { await reload(); failed = true }
    }
}
