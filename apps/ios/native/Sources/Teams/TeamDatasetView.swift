import SwiftUI

struct TeamDatasetSheet: View {
    private enum Cursor: Equatable {
        case start, after(String), block(Int)
    }

    private struct Page {
        let title: String
        let text: String
        let next: Cursor?
        var editable: [EditableRow] = []
    }

    private struct EditableRow: Identifiable {
        let dataset: String
        let id: String
        let revision: Int64
        let schema: Int64
        let partition: String
        let value: String
    }

    private struct Edit {
        let row: EditableRow
        var value: String
        var operation = UUID().uuidString.lowercased()
    }

    let reference: DatasetResourceRef
    let grant: String
    let conversation: String
    let model: TeamMessagingModel
    @Environment(\.dismiss) private var dismiss
    @State private var page: Page?
    @State private var cursor = Cursor.start
    @State private var attempt = 0
    @State private var failed = false
    @State private var saving = false
    @State private var conflict = false
    @State private var edit: Edit?

    var body: some View {
        NavigationStack {
            ScrollView {
                if let edit {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(edit.row.id).font(.headline)
                        TextEditor(text: editValue).font(.system(.body, design: .monospaced))
                            .frame(minHeight: 180)
                            .accessibilityLabel(Text(edit.row.id))
                            .disabled(saving)
                        if conflict { Text("teams_data_conflict").foregroundStyle(.red) }
                        else if failed { Text("teams_data_unavailable").foregroundStyle(.red) }
                        HStack {
                            if conflict { Button("common_retry") { attempt += 1 } }
                            else {
                                Button("common_cancel") { self.edit = nil; failed = false }.disabled(saving)
                                Button("teams_dataset_save_record") { Task { await save(edit) } }.disabled(saving)
                            }
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                } else if let page {
                    VStack(alignment: .leading, spacing: 16) {
                        if page.text.isEmpty { Text("teams_dataset_no_rows") }
                        else { Text(page.text).font(.system(.body, design: .monospaced)).textSelection(.enabled) }
                        ForEach(page.editable) { row in
                            Button { edit = Edit(row: row, value: row.value) } label: {
                                HStack { Text("teams_dataset_edit_record"); Text(row.id) }
                            }
                        }
                        if let next = page.next {
                            Button("teams_dataset_next") { cursor = next; attempt += 1 }
                        } else if case .dataset = reference {
                            Button("common_retry") { attempt += 1 }
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                } else if failed {
                    VStack {
                        Text("teams_data_unavailable")
                        Button("common_retry") { attempt += 1 }
                    }
                } else {
                    ProgressView()
                }
            }
            .padding()
            .navigationTitle(page.map { Text($0.title) } ?? Text("teams_dataset_title"))
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() }.disabled(saving) } }
        }
        .interactiveDismissDisabled(saving)
        .task(id: attempt) { await load() }
    }

    private func load() async {
        page = nil
        failed = false
        conflict = false
        edit = nil
        do {
            let (dataset, command) = try request()
            var editable = false
            if case .dataset = reference {
                let description = try await send(dataset: dataset, command: ["action": "describe"])
                editable = description["access"] as? String == "edit"
            }
            let json = try await send(dataset: dataset, command: command)
            let result = try decodePage(json, dataset: dataset, editable: editable)
            try Task.checkCancellation()
            page = result
        } catch is CancellationError {
            return
        } catch {
            if !Task.isCancelled { failed = true }
        }
    }

    private var invalid: TeamMessageFailure { TeamMessageFailure(code: "INVALID_DATA") }

    private var editValue: Binding<String> {
        Binding(get: { edit?.value ?? "" }, set: { value in
            edit?.value = value
            edit?.operation = UUID().uuidString.lowercased()
        })
    }

    private func send(dataset: String, command: [String: Any], operation: String? = nil) async throws -> [String: Any] {
        var body: [String: Any] = [
            "protocol": DatasetResourceRef.protocolVersion, "dataset": dataset, "via": grant, "command": command,
        ]
        if let operation { body["operation"] = operation }
        let data = try await model.dataset(conversationId: conversation, body: body)
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw invalid }
        return json
    }

    private func save(_ change: Edit) async {
        guard !saving else { return }
        saving = true
        failed = false
        defer { saving = false }
        do {
            guard let value = try JSONSerialization.jsonObject(with: Data(change.value.utf8)) as? [String: Any] else { throw invalid }
            let row = change.row
            _ = try await send(dataset: row.dataset, command: [
                "action": "put", "record": row.id, "partition": row.partition, "expectedRevision": row.revision,
                "schemaVersion": row.schema, "value": value,
            ], operation: change.operation)
            attempt += 1
        } catch {
            let code = (error as? TeamMessageFailure)?.code
            conflict = code == "DATA_CONFLICT"
            failed = true
            if ["INVALID_SESSION", "CONVERSATION_REQUIRED", "DATA_ACCESS_REQUIRED"].contains(code ?? "") {
                page = nil
                edit = nil
            }
        }
    }

    private func request() throws -> (String, [String: Any]) {
        var pagination: [String: Any] = [:]
        if case let .after(after) = cursor { pagination["after"] = after }
        switch reference {
        case let .dataset(dataset):
            pagination["action"] = "list"
            return (dataset, pagination)
        case let .record(dataset, _, _, _):
            let encoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(reference))
            return (dataset, ["action": "read", "reference": encoded])
        case let .snapshot(dataset, snapshot):
            let block: Int
            if case let .block(index) = cursor { block = index } else { block = 0 }
            return (dataset, ["action": "openSnapshot", "snapshot": snapshot, "block": block])
        case let .reportRelease(dataset, release):
            pagination["action"] = "openReport"
            pagination["release"] = release
            return (dataset, pagination)
        case .pageVersion:
            throw invalid
        }
    }

    private func decodePage(_ json: [String: Any], dataset: String, editable: Bool) throws -> Page {
        let after = (json["cursor"] as? String).map(Cursor.after)
        switch reference {
        case let .record(_, record, _, _):
            guard let received = json["reference"],
                  try JSONDecoder().decode(DatasetResourceRef.self, from: JSONSerialization.data(withJSONObject: received)) == reference,
                  let value = json["value"] as? [String: Any] else { throw invalid }
            return Page(title: record, text: try displayJSON(value), next: nil)
        case .reportRelease:
            guard let title = json["title"] as? String, let metric = json["metric"] as? [String: Any],
                  let groups = json["groups"] as? [[String: String]] else { throw invalid }
            let scale = metric["kind"] as? String == "sum" ? metric["scale"] as? Int ?? -1 : 0
            guard (0...12).contains(scale) else { throw invalid }
            let text = try groups.map { group -> String in
                guard let label = group["group"], let units = group["units"] else { throw invalid }
                return label + "  " + decimal(units, scale: scale)
            }.joined(separator: "\n\n")
            return Page(title: title, text: text, next: after)
        case .dataset, .snapshot:
            guard let records = json["records"] as? [[String: Any]] else { throw invalid }
            var changes: [EditableRow] = []
            let text = try records.map { row -> String in
                guard let wire = row["reference"] as? [String: Any], let key = wire["record"] as? String,
                      let value = row["value"] as? [String: Any] else { throw invalid }
                let display = try displayJSON(value)
                if editable && row["archived"] as? Bool != true {
                    let record = try JSONDecoder().decode(DatasetResourceRef.self, from: JSONSerialization.data(withJSONObject: wire))
                    guard case let .record(actual, _, revision, schema) = record, actual == dataset,
                          let partition = row["partition"] as? String else { throw invalid }
                    changes.append(EditableRow(dataset: dataset, id: key, revision: revision, schema: schema, partition: partition, value: display))
                }
                return key + "\n" + display
            }.joined(separator: "\n\n")
            return Page(title: dataset, text: text, next: (json["nextBlock"] as? Int).map(Cursor.block) ?? after, editable: changes)
        case .pageVersion:
            throw invalid
        }
    }

    private func displayJSON(_ value: [String: Any]) throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys]), as: UTF8.self)
    }

    private func decimal(_ units: String, scale: Int) -> String {
        guard scale > 0 else { return units }
        let sign = units.hasPrefix("-") ? "-" : ""
        let digits = sign.isEmpty ? units : String(units.dropFirst())
        let padded = String(repeating: "0", count: max(0, scale + 1 - digits.count)) + digits
        return sign + padded.dropLast(scale) + "." + padded.suffix(scale)
    }
}
