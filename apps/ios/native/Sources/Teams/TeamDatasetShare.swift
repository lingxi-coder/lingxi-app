import SwiftUI

struct TeamDatasetShareSheet: View {
    private enum Mode: String, CaseIterable {
        case pinned, live, edit

        var label: LocalizedStringKey {
            switch self {
            case .pinned: "teams_dataset_fixed"
            case .live: "teams_dataset_live"
            case .edit: "teams_dataset_edit"
            }
        }
    }

    private struct Choice {
        let dataset: String
        let title: String
        let fields: [String]
        let digest: String
        let people: [String]
    }

    let context: TeamConversationContext
    @Environment(\.dismiss) private var dismiss
    @State private var items: [String] = []
    @State private var cursor: String?
    @State private var choice: Choice?
    @State private var fields: Set<String> = []
    @State private var mode = Mode.pinned
    @State private var operation: String?
    @State private var busy = false
    @State private var failed = false

    var body: some View {
        NavigationStack {
            Form {
                if failed { Text("teams_data_unavailable").foregroundStyle(.red) }
                if let choice {
                    Section {
                        Text(choice.title).font(.headline)
                        Text("teams_dataset_audience \(choice.people.map(personName).joined(separator: " · "))")
                        Picker("teams_dataset_access", selection: $mode) {
                            ForEach(Mode.allCases, id: \.self) { Text($0.label).tag($0) }
                        }.disabled(busy || operation != nil)
                        ForEach(choice.fields, id: \.self) { field in
                            Toggle(field, isOn: Binding(get: { fields.contains(field) }, set: { shown in
                                if shown { fields.insert(field) } else { fields.remove(field) }
                            })).disabled(busy || operation != nil)
                        }
                    }
                    Button(operation == nil ? LocalizedStringKey("teams_dataset_share") : "teams_dataset_continue") {
                        Task { await publish(choice) }
                    }.disabled(busy || fields.isEmpty)
                } else {
                    ForEach(items, id: \.self) { dataset in
                        Button(dataset) { Task { await select(dataset) } }.disabled(busy)
                    }
                    if let cursor {
                        Button("teams_dataset_next") { Task { await discover(after: cursor) } }.disabled(busy)
                    }
                    if items.isEmpty && !busy {
                        if failed { Button("common_retry") { Task { await discover(after: nil) } } }
                        else { Text("teams_dataset_no_rows") }
                    }
                }
                if busy { ProgressView() }
            }
            .navigationTitle("teams_dataset_title")
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() }.disabled(busy) } }
        }
        .interactiveDismissDisabled(busy)
        .task { await discover(after: nil) }
    }

    private var destination: [String: String] { ["team": context.team.id, "conversation": context.conversation.id] }
    private var invalid: TeamMessageFailure { TeamMessageFailure(code: "INVALID_DATA") }

    private func personName(_ uid: String) -> String {
        guard let name = context.members.first(where: { $0.uid == uid })?.displayName, !name.isEmpty else { return uid }
        return name
    }

    private func send(_ body: [String: Any]) async throws -> [String: Any] {
        let data = try await context.model.dataset(conversationId: context.conversation.id, body: body)
        guard let response = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw invalid }
        return response
    }

    private func read(_ dataset: String, _ command: [String: Any]) async throws -> [String: Any] {
        try await send(["protocol": DatasetResourceRef.protocolVersion, "dataset": dataset, "command": command])
    }

    private func handling(_ work: () async throws -> Void) async {
        guard !busy else { return }
        busy = true
        failed = false
        defer { busy = false }
        do { try await work() }
        catch { if !Task.isCancelled { failed = true } }
    }

    private func discover(after: String?) async {
        await handling {
            var query: [String: Any] = ["action": "discover", "scope": ["kind": "mine"]]
            if let after { query["after"] = after }
            let page = try await read("", query)
            guard let rows = page["resources"] as? [[String: Any]] else { throw invalid }
            let ids = rows.compactMap { row -> String? in
                guard row["canShare"] as? Bool == true, row["via"] == nil || row["via"] is NSNull,
                      let reference = row["reference"] as? [String: Any], reference["kind"] as? String == "dataset" else { return nil }
                return reference["dataset"] as? String
            }
            try Task.checkCancellation()
            items = after == nil ? ids : items + ids.filter { !items.contains($0) }
            cursor = page["cursor"] as? String
        }
    }

    private func select(_ dataset: String) async {
        await handling {
            let description = try await read(dataset, ["action": "describe"])
            let audience = try await read(dataset, ["action": "audience", "destination": destination])
            guard let schema = description["schema"] as? [String: Any], let properties = schema["properties"] as? [String: Any],
                  !properties.isEmpty, let digest = audience["audienceDigest"] as? String,
                  let readers = audience["audience"] as? [String: Int] else { throw invalid }
            try Task.checkCancellation()
            choice = Choice(dataset: dataset, title: schema["title"] as? String ?? dataset,
                            fields: properties.keys.sorted(), digest: digest, people: readers.keys.sorted())
            fields = Set(properties.keys)
        }
    }

    private func publish(_ choice: Choice) async {
        await handling {
            let key = operation ?? UUID().uuidString.lowercased()
            operation = key
            var request: [String: Any] = ["action": "share", "dataset": choice.dataset, "operation": key, "destination": destination,
                "generation": context.team.generation, "audienceDigest": choice.digest, "text": choice.title,
                "mode": mode.rawValue, "projection": ["fields": fields.sorted()]]
            if context.conversation.kind == "bot_dm" {
                guard let bot = context.conversation.effectiveSharedBotId else { throw invalid }
                request["sharedBot"] = bot
            }
            if try await send(request)["status"] as? String == "published" { dismiss() }
        }
    }
}
