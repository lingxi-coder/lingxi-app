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
    @State private var previewHeight: CGFloat = 48
    @State private var audience: [String: Any] = [:]
    @State private var location = "mine"
    @State private var sources: [[String: Any]] = []
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
                    }
                }
                TextField("teams_visualization_title", text: $title)
                Button("teams_visualization_import") { importing = true }
                TextEditor(text: $html)
                    .font(.body.monospaced()).frame(minHeight: 110)
                    .accessibilityLabel("HTML")
                Button("teams_visualization_preview") { preview.toggle() }.disabled(!valid)
                if preview {
                    TeamVisualizationWebSurface(load: ["bundle": bundle, "aliases": [String](),
                        "locale": locale.identifier, "theme": ["mode": colorScheme == .dark ? "dark" : "light", "tokens": [:]]],
                        receive: { value in
                            if let height = value["height"] as? Double, height.isFinite { previewHeight = CGFloat(max(32, min(360, height))) }
                            if value["type"] as? String == "error" { error = "VISUALIZATION_SCRIPT_FAILED" }
                        }, connected: { _ in })
                        .frame(height: previewHeight)
                }
                dataSection
                if let members = audience["audience"] as? [String: Int] {
                    LabeledContent("teams_visualization_audience", value: members.keys.sorted().map(person).joined(separator: " · "))
                }
                if let error { Text(error).foregroundStyle(.red) }
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
                    guard let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size <= 2_097_152 else {
                        throw TeamMessageFailure(code: "VISUALIZATION_TOO_LARGE")
                    }
                    html = try String(contentsOf: url, encoding: .utf8)
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

    private func parseList(_ value: String) -> [String] {
        Set(value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }).sorted()
    }

    private func loadAudience() async {
        error = nil
        do {
            let result = try await model.visualization(conversationId: destination, body: ["action": "audience"])
            try Task.checkCancellation()
            audience = result
        } catch is CancellationError { }
        catch { self.error = (error as? TeamMessageFailure)?.code ?? "VISUALIZATION_UNAVAILABLE" }
    }

    private func discover() async {
        index = -1
        sources = []
        let target = model.datasetDestination(conversationId: destination)
        var scope: [String: Any] = ["kind": "mine"]
        if location == "team" { scope["team"] = target["team"] }
        if location == "shared" { scope = ["kind": "conversation", "destination": target] }
        do {
            let data = try await model.dataset(conversationId: destination, body: ["protocol": DatasetResourceRef.protocolVersion,
                "command": ["action": "discover", "scope": scope, "limit": 50]])
            try Task.checkCancellation()
            let value = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            sources = (value?["resources"] as? [[String: Any]] ?? []).filter {
                location == "shared" ? $0["via"] is String : $0["canShare"] as? Bool == true
            }
        } catch { /* HTML can be shared independently of the Data Core connection. */ }
    }

    private func publish() async {
        guard !busy, let digest = audience["audienceDigest"] as? String else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            var body: [String: Any] = ["action": "publish", "bundle": bundle, "bindings": bindings, "audienceDigest": digest]
            let encoded = try JSONSerialization.data(withJSONObject: body, options: .sortedKeys)
            guard encoded.count <= 2_105_344 else { throw TeamMessageFailure(code: "VISUALIZATION_TOO_LARGE") }
            if intent?.0 != encoded { intent = (encoded, UUID().uuidString) }
            body["operation"] = intent?.1
            let result = try await model.visualization(conversationId: destination, body: body)
            if result["status"] as? String == "building" { building = true } else { dismiss() }
        } catch {
            self.error = (error as? TeamMessageFailure)?.code ?? "DATA_UNAVAILABLE"
            if self.error == "DATA_SHARE_CHANGED" { await loadAudience() }
        }
    }
}
