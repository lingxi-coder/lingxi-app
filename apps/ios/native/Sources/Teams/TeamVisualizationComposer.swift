import SwiftUI
import UniformTypeIdentifiers

struct TeamVisualizationComposer: View {
    let conversation: String
    let model: TeamMessagingModel
    let person: (String) -> String
    var source: [String: Any]? = nil
    var destinations: [TeamMessageConversation] = []

    @Environment(\.dismiss) private var dismiss
    @Environment(\.locale) private var locale
    @Environment(\.colorScheme) private var colorScheme
    @State private var target: String?
    @State private var title = ""
    @State private var html = ""
    @State private var importing = false
    @State private var preview = false
    @State private var previewSend: (([String: Any]) -> Void)?
    @State private var previewHeight: CGFloat = 48
    @State private var audience: [String: Any] = [:]
    @State private var location = "mine"
    @State private var sources: [[String: Any]] = []
    @State private var cursor: String?
    @State private var discoveryId = UUID()
    @State private var discovering = false
    @State private var index = -1
    @State private var alias = ""
    @State private var mode = "pinned"
    @State private var fields = ""
    @State private var partitions = ""
    @State private var bindings: [[String: Any]] = []
    @State private var intent: (Data, String)?
    @State private var busy = false
    @State private var building = false
    @State private var error: String?

    private var bundle: [String: Any] {
        ["title": title.trimmingCharacters(in: .whitespacesAndNewlines), "html": html,
         "assets": source?["assets"] ?? [String: Any]()]
    }
    private var destination: String { target ?? conversation }
    private var valid: Bool { !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && title.count <= 80 && !html.isEmpty }
    private var selected: [String: Any]? { sources.indices.contains(index) ? sources[index] : nil }

    var body: some View {
        NavigationStack {
            Form {
                if !destinations.isEmpty {
                    Picker("teams_visualization_destination", selection: Binding(get: { destination }, set: {
                        target = $0; audience = [:]; bindings = []; preview = false
                    })) {
                        ForEach(destinations) { Text($0.title).tag($0.id) }
                    }.disabled(busy)
                }
                TextField("teams_visualization_title", text: $title)
                Button("teams_visualization_import") { importing = true }
                if html.utf8.count > TeamVisualizationTransfer.editableChars {
                    LabeledContent("HTML", value: ByteCountFormatter.string(fromByteCount: Int64(html.utf8.count), countStyle: .binary))
                } else {
                    TextEditor(text: $html)
                        .font(.body.monospaced()).frame(minHeight: 110)
                        .accessibilityLabel("HTML")
                }
                Button("teams_visualization_preview") { preview.toggle() }.disabled(!valid)
                if preview {
                    let aliases = bindings.compactMap { $0["alias"] as? String }
                    TeamVisualizationWebSurface(load: ["bundle": bundle, "aliases": aliases,
                        "locale": locale.identifier(.bcp47), "theme": ["mode": colorScheme == .dark ? "dark" : "light", "tokens": [:]]],
                        receive: { value in
                            if let height = value["height"] as? Double, height.isFinite { previewHeight = CGFloat(max(32, min(360, height))) }
                            if value["type"] as? String == "error" { error = "VISUALIZATION_SCRIPT_FAILED" }
                            respondToPreview(value, aliases: aliases)
                        }, connected: { previewSend = $0 })
                        .id(aliases)
                        .frame(height: previewHeight)
                        .onDisappear { previewSend = nil }
                }
                dataSection
                if let members = audience["audience"] as? [String: Int] {
                    LabeledContent("teams_visualization_audience", value: members.keys.sorted().map(person).joined(separator: " · "))
                }
                if let error {
                    Text(error == "VISUALIZATION_TOO_LARGE" ? String(localized: "teams_visualization_too_large") : error).foregroundStyle(.red)
                }
                if building { Text("teams_visualization_building") }
            }
            .navigationTitle(destinations.isEmpty ? "teams_visualization_add" : "teams_visualization_forward")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() }.disabled(busy) }
                ToolbarItem(placement: .confirmationAction) {
                    Button(building ? "teams_visualization_retry" : "teams_visualization_send") { Task { await publish() } }
                        .disabled(!valid || audience["audienceDigest"] == nil || selected != nil || busy)
                }
            }
            .task {
                title = source?["title"] as? String ?? ""
                html = source?["html"] as? String ?? ""
            }
            .task(id: destination) { await loadAudience() }
            .task(id: destination + ":" + location) { await discover() }
            .onChange(of: index) { _, _ in select() }
            .onChange(of: html) { _, _ in preview = false }
            .fileImporter(isPresented: $importing, allowedContentTypes: [.html]) { response in
                do {
                    let url = try response.get()
                    let accessible = url.startAccessingSecurityScopedResource()
                    defer { if accessible { url.stopAccessingSecurityScopedResource() } }
                    let file = try FileHandle(forReadingFrom: url)
                    defer { try? file.close() }
                    let maximum = TeamVisualizationTransfer.maximumBytes
                    guard let bytes = try file.read(upToCount: maximum + 1), bytes.count <= maximum else {
                        throw TeamMessageFailure(code: "VISUALIZATION_TOO_LARGE")
                    }
                    guard let markup = String(data: bytes, encoding: .utf8) else { throw TeamMessageFailure(code: "INVALID_DATA") }
                    html = markup
                    if title.isEmpty { title = String(url.deletingPathExtension().lastPathComponent.prefix(80)) }
                } catch { self.error = (error as? TeamMessageFailure)?.code ?? "INVALID_DATA" }
            }
            .interactiveDismissDisabled(busy)
        }
    }

    private var dataSection: some View {
        Section("teams_dataset_title") {
            Picker("teams_visualization_source", selection: $location) {
                Text("teams_visualization_personal").tag("mine")
                Text("teams_visualization_team").tag("team")
                Text("teams_visualization_shared").tag("shared")
            }
            Picker("teams_dataset_title", selection: $index) {
                Text("common_none").tag(-1)
                ForEach(sources.indices, id: \.self) { i in Text(datasetId(sources[i])).tag(i) }
            }
            if let cursor {
                Button("teams_visualization_more") { Task { await discover(after: cursor) } }.disabled(discovering)
            }
            if let selected {
                TextField("teams_visualization_alias", text: $alias)
                    .autocorrectionDisabled()
                Picker("teams_visualization_access", selection: $mode) {
                    if location != "shared" || selected["access"] as? String == "pinned" { Text("teams_dataset_open_fixed").tag("pinned") }
                    if selected["access"] as? String != "pinned" { Text("teams_dataset_open_live").tag("live") }
                    if selected["access"] as? String == "edit" { Text("teams_visualization_edit").tag("edit") }
                }
                if location != "shared" {
                    TextField("teams_visualization_fields", text: $fields).autocorrectionDisabled()
                    TextField("teams_visualization_partitions", text: $partitions).autocorrectionDisabled()
                }
                Button("teams_dataset_title") { addBinding() }
                    .disabled(bindings.count >= 4 || bindings.contains { $0["alias"] as? String == alias }
                              || alias.range(of: "^[A-Za-z0-9_-]{1,64}$", options: .regularExpression) == nil)
            }
            ForEach(bindings.indices, id: \.self) { i in
                HStack {
                    Text(bindings[i]["alias"] as? String ?? "")
                    Spacer()
                    Button("teams_visualization_remove", role: .destructive) { bindings.remove(at: i) }
                }
            }
        }
    }

    private func datasetId(_ item: [String: Any]) -> String {
        (item["reference"] as? [String: Any])?["dataset"] as? String ?? ""
    }

    private func select() {
        alias = selected.map(datasetId) ?? ""
        mode = location == "shared" ? selected?["access"] as? String ?? "pinned" : "pinned"
        fields = ""; partitions = ""
    }

    private func addBinding() {
        guard let selected, let reference = selected["reference"] as? [String: Any] else { return }
        var binding: [String: Any] = ["alias": alias, "mode": mode]
        if location == "shared", let grant = selected["via"] as? String {
            binding["kind"] = "reuse"; binding["grant"] = grant; binding["reference"] = reference
        } else {
            binding["kind"] = "share"; binding["dataset"] = datasetId(selected)
            var projection: [String: [String]] = [:]
            let names = parseList(fields), parts = parseList(partitions)
            if !names.isEmpty { projection["fields"] = names }
            if !parts.isEmpty { projection["partitions"] = parts }
            binding["projection"] = projection
            if reference["kind"] as? String != "dataset" { binding["reference"] = reference }
        }
        bindings.append(binding)
        index = -1
    }

    private func respondToPreview(_ packet: [String: Any], aliases: [String]) {
        guard packet["type"] as? String == "request", let id = packet["id"] as? Int else { return }
        let action = packet["action"] as? String
        var result: [String: Any] = ["id": id]
        if action == "state", var state = packet["state"] as? [String: Any],
           let expected = packet["expected"] as? Int, expected < Int.max {
            state["version"] = expected + 1
            result["value"] = state
        } else if action == "unwatch" {
            result["value"] = NSNull()
        } else if (action == "read" || action == "watch"), let alias = packet["alias"] as? String, aliases.contains(alias) {
            result["value"] = ["records": [[String: Any]]()]
        } else {
            result["error"] = "DATA_ACCESS_REQUIRED"
        }
        previewSend?(["type": "result", "value": result])
    }

    private func parseList(_ value: String) -> [String] {
        Set(value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }).sorted()
    }

    private func loadAudience() async {
        error = nil
        do {
            let result = try await model.visualization(conversationId: destination, body: ["action": "audience"], from: conversation)
            try Task.checkCancellation()
            audience = result
        } catch is CancellationError { }
        catch { self.error = (error as? TeamMessageFailure)?.code ?? "VISUALIZATION_UNAVAILABLE" }
    }

    private func discover(after: String? = nil) async {
        let request = UUID(), channel = destination, source = location
        discoveryId = request
        discovering = true
        defer { if discoveryId == request { discovering = false } }
        if after == nil { index = -1; sources = []; cursor = nil }
        let target = model.datasetDestination(conversationId: channel)
        var scope: [String: Any] = ["kind": "mine"]
        if source == "team" { scope["team"] = target["team"] }
        if source == "shared" { scope = ["kind": "conversation", "destination": target] }
        var command: [String: Any] = ["action": "discover", "scope": scope, "limit": 50]
        command["after"] = after
        do {
            let data = try await model.dataset(conversationId: channel, body: ["protocol": DatasetResourceRef.protocolVersion,
                "command": command], from: conversation)
            try Task.checkCancellation()
            guard discoveryId == request, destination == channel, location == source else { return }
            let value = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            let found = (value?["resources"] as? [[String: Any]] ?? []).filter {
                source == "shared" ? $0["via"] is String : $0["canShare"] as? Bool == true
            }
            if after == nil { sources = found } else { sources += found }
            cursor = value?["cursor"] as? String
        } catch { /* Standalone HTML can be shared independently of the Data Core connection. */ }
    }

    private func publish() async {
        guard !busy, let digest = audience["audienceDigest"] as? String else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            let transfer = try TeamVisualizationTransfer.prepare(bundle)
            var body: [String: Any] = ["action": "publish", "package": transfer.manifest, "bindings": bindings, "audienceDigest": digest]
            let encoded = try JSONSerialization.data(withJSONObject: body, options: .sortedKeys)
            if intent?.0 != encoded { intent = (encoded, UUID().uuidString) }
            body["operation"] = intent?.1
            let target = destination
            let request: TeamVisualizationTransfer.Request = { packet in
                try await model.visualization(conversationId: target, body: packet, from: conversation)
            }
            body["stage"] = "prepare"
            var result = try await request(body)
            if result["status"] as? String == "uploading" {
                try await TeamVisualizationTransfer.upload(transfer, plan: result, request: request)
                body["stage"] = "commit"
                result = try await request(body)
            }
            if result["status"] as? String == "building" { building = true } else { dismiss() }
        } catch {
            self.error = (error as? TeamMessageFailure)?.code ?? "DATA_UNAVAILABLE"
            if self.error == "DATA_SHARE_CHANGED" {
                intent = nil
                building = false
                await loadAudience()
                if self.error == nil { self.error = "DATA_SHARE_CHANGED" }
            }
        }
    }
}
