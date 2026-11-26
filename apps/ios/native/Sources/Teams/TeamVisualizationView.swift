import SwiftUI
import WebKit

struct TeamVisualizationReference: Decodable, Equatable, Sendable {
    struct Binding: Decodable, Equatable, Sendable {
        let alias: String
        let mode: String
    }

    let reference: DatasetResourceRef
    let title: String
    let bindings: [Binding]

    var page: (id: String, version: String)? {
        guard case let .pageVersion(id, version) = reference, id.hasPrefix("viz_"),
              !title.isEmpty, title.count <= 80, bindings.count <= 4,
              Set(bindings.map(\.alias)).count == bindings.count else { return nil }
        return (id, version)
    }
}

extension View {
    fileprivate func visualizationPanel() -> some View {
        padding(10).background(.background, in: RoundedRectangle(cornerRadius: 10))
    }
}

struct TeamVisualizationView: View {
    let reference: TeamVisualizationReference
    let conversation: String
    let model: TeamMessagingModel
    var destinations: [TeamMessageConversation] = []
    var person: (String) -> String = { $0 }

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.locale) private var locale
    @Environment(\.scenePhase) private var scenePhase
    @State private var payload: [String: Any]?
    @State private var height: CGFloat = 44
    @State private var expanded = false
    @State private var forward = false
    @State private var error: String?
    @State private var attempt = 0
    @State private var mountID = UUID()
    @State private var ship: (([String: Any]) -> Void)?
    @State private var watches: [String: [String: Any]] = [:]
    @State private var watchSubscriptions: [String: WatchSubscription] = [:]
    @State private var refreshRequested: Set<String> = []
    @State private var edit: Edit?
    @State private var handlingEdit = false

    private struct Edit: Identifiable {
        let id = UUID()
        let alias: String
        let record: String
        let before: [String: Any]
        let changes: [String: Any]
        let confirmed: (Bool) -> Void
        let conflict: Bool
    }

    private var theme: [String: Any] { ["mode": colorScheme == .dark ? "dark" : "light", "tokens": [String: String]()] }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(reference.title).font(.subheadline.weight(.semibold))
                Spacer()
                if payload != nil {
                    if !destinations.isEmpty {
                        Button("teams_visualization_forward") { forward = true }.font(.footnote)
                    }
                    Button(expanded ? "teams_visualization_collapse" : "teams_visualization_expand") { expanded.toggle() }
                        .font(.footnote)
                }
            }
            if let payload, error == nil {
                TeamVisualizationWebSurface(load: payload, receive: handle, connected: { ship = $0 })
                    .frame(height: min(height, expanded ? 880 : 520))
            }
            if let error {
                Text(error).font(.footnote).foregroundStyle(.secondary)
                Button("teams_visualization_retry") { attempt += 1 }
            }
        }
        .visualizationPanel()
        .accessibilityIdentifier("teams.visualization." + (reference.page?.id ?? "unavailable"))
        .task(id: TaskKey(offline: model.offline, attempt: attempt, foreground: scenePhase == .active)) { await mount() }
        .onChange(of: expanded) { _, value in ship?(["type": "layout", "value": ["expanded": value]]) }
        .onChange(of: colorScheme) { _, _ in ship?(["type": "layout", "value": ["theme": theme]]) }
        .onDisappear { retire() }
        .sheet(isPresented: $forward) {
            if let source = payload?["bundle"] as? [String: Any] {
                TeamVisualizationComposer(conversation: conversation, model: model, person: person, source: source, destinations: destinations)
            }
        }
        .sheet(item: $edit) { proposal in editSheet(proposal).interactiveDismissDisabled() }
    }

    private struct TaskKey: Equatable { let offline: Bool; let attempt: Int; let foreground: Bool }
    private struct WatchSubscription: Equatable {
        let guestID: Int
        let token = UUID()
    }

    private func retire() {
        mountID = UUID()
        ship?(["type": "suspend"])
        ship = nil
        payload = nil
        watches.removeAll()
        watchSubscriptions.removeAll()
        refreshRequested.removeAll()
        cancelEdit()
    }

    private func cancelEdit() {
        edit?.confirmed(false)
        edit = nil
    }

    private func request(_ action: String, fields: [String: Any] = [:]) async throws -> [String: Any] {
        guard let page = reference.page else { throw TeamMessageFailure(code: "INVALID_DATA") }
        var body = fields
        body["action"] = action; body["message"] = page.id; body["version"] = page.version
        return try await model.visualization(conversationId: conversation, body: body)
    }

    private func data(_ alias: String, _ action: String, fields: [String: Any] = [:], operation: String? = nil) async throws -> [String: Any] {
        var input = fields
        input["action"] = action
        var body: [String: Any] = ["alias": alias, "request": input]
        body["operation"] = operation
        return try await request("data", fields: body)
    }

    private func mount() async {
        retire()
        guard !model.offline, scenePhase == .active, let page = reference.page else { return }
        let mounted = mountID
        error = nil
        do {
            let opened = try await request("open")
            try Task.checkCancellation()
            guard mountID == mounted else { return }
            guard let bundle = opened["bundle"], let state = opened["state"] else {
                throw TeamMessageFailure(code: "INVALID_DATA")
            }
            let detach = model.observeVisualization(conversation: conversation, message: page.id, version: page.version)
            defer { detach() }
            payload = ["bundle": bundle, "aliases": reference.bindings.map(\.alias),
                       "state": state, "theme": theme, "locale": locale.identifier(.bcp47), "expanded": expanded]
            let clock = ContinuousClock()
            var nextPoll: [String: ContinuousClock.Instant] = [:]
            while !Task.isCancelled && mountID == mounted {
                try await Task.sleep(for: .milliseconds(Int.random(in: 3_000...3_500)))
                guard mountID == mounted else { return }
                try await model.checkVisualization(conversation: conversation, message: page.id, version: page.version)
                let due = watches.compactMap { alias, snapshot -> (String, [String: Any], WatchSubscription)? in
                    guard let subscription = watchSubscriptions[alias],
                          refreshRequested.contains(alias) || (nextPoll[alias].map { $0 <= clock.now } ?? true) else { return nil }
                    return (alias, snapshot, subscription)
                }
                for (alias, snapshot, subscription) in due {
                    guard watchSubscriptions[alias] == subscription else { continue }
                    refreshRequested.remove(alias)
                    do {
                        let (next, changed) = try await refresh(alias: alias, previous: snapshot)
                        try Task.checkCancellation()
                        guard watchSubscriptions[alias] == subscription else { continue }
                        if changed, let values = next["data"] as? NSDictionary, !values.isEqual(snapshot["data"]) {
                            ship?(["type": "data", "value": ["alias": alias, "subscription": subscription.guestID, "value": values]])
                        }
                        watches[alias] = next
                        nextPoll[alias] = clock.now.advanced(by: .seconds(next["cursors"] is [String] ? changed ? 3 : 12 : 30))
                    } catch {
                        if watchSubscriptions[alias] == subscription { throw error }
                    }
                }
            }
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled else { return }
            retire()
            self.error = String(localized: "teams_data_unavailable")
        }
    }

    private func refresh(alias: String, previous: [String: Any]) async throws -> ([String: Any], Bool) {
        if let cursors = previous["cursors"] as? [String] {
            do {
                let response = try await data(alias, "changes", fields: ["cursors": cursors])
                if response["changed"] as? Bool == false && response["schemaVersion"] as? Int == previous["schemaVersion"] as? Int {
                    var snapshot = previous
                    snapshot["cursors"] = response["cursors"]
                    return (snapshot, false)
                }
            } catch let failure as TeamMessageFailure where ["DATA_BACKFILL_REQUIRED", "INVALID_DATA_REQUEST"].contains(failure.code) {
                // The next read establishes a new cursor under the current grant.
            }
        }
        return (try await data(alias, "read", fields: ["watch": true]), true)
    }

    private func handle(_ packet: [String: Any]) {
        switch packet["type"] as? String {
        case "height":
            if let value = packet["height"] as? Double, value.isFinite { height = CGFloat(min(4096, max(32, value))) }
        case "error":
            retire()
            error = String(localized: "teams_data_unavailable")
        case "request":
            guard let id = packet["id"] as? Int else { return }
            let receiver = mountID
            Task { @MainActor in
                guard mountID == receiver else { return }
                do {
                    let result = try await respond(packet)
                    guard mountID == receiver else { return }
                    ship?(["type": "result", "value": ["id": id, "value": result]])
                } catch {
                    guard mountID == receiver else { return }
                    ship?(["type": "result", "value": ["id": id, "error": (error as? TeamMessageFailure)?.code ?? "DATA_UNAVAILABLE"]])
                }
            }
        default: break
        }
    }

    private func respond(_ packet: [String: Any]) async throws -> Any {
        guard payload != nil, let action = packet["action"] as? String else { throw TeamMessageFailure(code: "DATA_ACCESS_REQUIRED") }
        if action == "expand" { expanded = true; return NSNull() }
        if action == "state", let state = packet["state"], let expected = packet["expected"] as? Int {
            return try await request("state", fields: ["state": state, "expected": expected])
        }
        guard let alias = packet["alias"] as? String, reference.bindings.contains(where: { $0.alias == alias }) else {
            throw TeamMessageFailure(code: "DATA_ACCESS_REQUIRED")
        }
        if (action == "watch" || action == "unwatch"), let guestID = packet["subscription"] as? Int {
            if action == "unwatch" {
                if watchSubscriptions[alias]?.guestID == guestID {
                    watchSubscriptions[alias] = nil
                    watches[alias] = nil
                    refreshRequested.remove(alias)
                }
                return NSNull()
            }
            if watchSubscriptions[alias]?.guestID == guestID, let saved = watches[alias]?["data"] { return saved }
            let subscription = WatchSubscription(guestID: guestID)
            watchSubscriptions[alias] = subscription
            watches[alias] = nil
            do {
                let snapshot = try await data(alias, "read", fields: ["watch": true])
                if watchSubscriptions[alias] == subscription {
                    watches[alias] = snapshot
                    refreshRequested.insert(alias)
                }
                return snapshot["data"] ?? NSNull()
            } catch {
                if watchSubscriptions[alias] == subscription { watchSubscriptions[alias] = nil }
                throw error
            }
        }
        if action == "read" {
            let fields = packet.filter { ["after", "block"].contains($0.key) && !($0.value is NSNull) }
            return try await data(alias, "read", fields: fields)["data"] ?? NSNull()
        }
        if action == "patch", reference.bindings.contains(where: { $0.alias == alias && $0.mode == "edit" }),
           let patch = packet["changes"] as? [String: Any], let record = patch["record"] as? String,
           let updates = patch["changes"] as? [String: Any], !updates.isEmpty, !handlingEdit {
            handlingEdit = true
            defer { handlingEdit = false }
            return try await confirmPatch(alias: alias, record: record, updates: updates)
        }
        throw TeamMessageFailure(code: "INVALID_DATA_REQUEST")
    }

    private func confirmPatch(alias: String, record: String, updates: [String: Any]) async throws -> [String: Any] {
        let receiver = mountID
        for attempt in 0..<2 {
            let snapshot = try await data(alias, "read", fields: ["record": record])
            guard mountID == receiver, payload != nil, snapshot["access"] as? String == "edit",
                  let records = (snapshot["data"] as? [String: Any])?["records"] as? [[String: Any]],
                  let recordData = records.first, recordData["archived"] as? Bool != true,
                  let revision = (recordData["reference"] as? [String: Any])?["revision"],
                  let schema = snapshot["schemaVersion"], let partition = recordData["partition"],
                  let value = recordData["value"] as? [String: Any] else {
                throw TeamMessageFailure(code: "DATA_ACCESS_REQUIRED")
            }
            let accepted = await withCheckedContinuation { continuation in
                edit = Edit(alias: alias, record: record, before: value, changes: updates,
                            confirmed: { continuation.resume(returning: $0) }, conflict: attempt != 0)
            }
            guard accepted, mountID == receiver, payload != nil else { throw TeamMessageFailure(code: "DATA_CANCELLED") }
            do {
                let result = try await data(alias, "patch", fields: ["record": record, "changes": updates,
                    "expectedRevision": revision, "schemaVersion": schema, "partition": partition], operation: UUID().uuidString)
                if watches[alias] != nil { refreshRequested.insert(alias) }
                return result
            } catch let failure as TeamMessageFailure where failure.code == "DATA_CONFLICT" && attempt == 0 {
                continue
            }
        }
        throw TeamMessageFailure(code: "DATA_CONFLICT")
    }

    private func editSheet(_ proposal: Edit) -> some View {
        NavigationStack {
            List {
                if proposal.conflict { Text("teams_visualization_conflict") }
                Text(proposal.alias + " · " + proposal.record)
                ForEach(proposal.changes.keys.sorted(), id: \.self) { field in
                    LabeledContent(field, value: display(proposal.before[field]) + " → " + display(proposal.changes[field]))
                }
            }
            .navigationTitle("teams_visualization_confirm")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { cancelEdit() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("teams_visualization_apply") {
                        edit = nil
                        proposal.confirmed(true)
                    }
                }
            }
        }
    }

    private func display(_ value: Any?) -> String {
        guard let value, let encoded = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed]),
              let text = String(data: encoded, encoding: .utf8) else { return "—" }
        return text
    }
}

struct TeamVisualizationWebSurface: UIViewRepresentable {
    let load: [String: Any]
    let receive: ([String: Any]) -> Void
    let connected: (@escaping ([String: Any]) -> Void) -> Void

    static let name = "teamVisualization"

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.userContentController.add(context.coordinator, name: Self.name)
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = context.coordinator
        view.isOpaque = false
        context.coordinator.webView = view
        context.coordinator.load = load
        context.coordinator.receive = receive
        context.coordinator.connected = connected
        if let url = Bundle.main.url(forResource: "team-visualization", withExtension: "html"),
           let html = try? String(contentsOf: url, encoding: .utf8) {
            view.loadHTMLString(html, baseURL: nil)
        }
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.receive = receive
    }

    static func dismantleUIView(_ view: WKWebView, coordinator: Coordinator) {
        view.stopLoading()
        view.configuration.userContentController.removeScriptMessageHandler(forName: name)
        coordinator.webView = nil
    }

    @MainActor final class Coordinator: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
        weak var webView: WKWebView?
        var load: [String: Any] = [:]
        var receive: (([String: Any]) -> Void)?
        var connected: ((@escaping ([String: Any]) -> Void) -> Void)?
        private var didStart = false
        private var beganNavigation = false

        func webView(_ view: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
            guard !beganNavigation, navigationAction.targetFrame?.isMainFrame == true,
                  navigationAction.request.url?.absoluteString == "about:blank" else { return .cancel }
            beganNavigation = true
            return .allow
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.frameInfo.isMainFrame, message.webView === webView, message.name == TeamVisualizationWebSurface.name,
                  let input = message.body as? String, input.utf8.count <= 131_072,
                  let value = try? JSONSerialization.jsonObject(with: Data(input.utf8)) as? [String: Any] else { return }
            if value["type"] as? String == "ready", !didStart {
                didStart = true
                send(["type": "initialize", "value": load])
                connected? { [weak self] packet in self?.send(packet) }
            } else if didStart {
                receive?(value)
            }
        }

        private func send(_ value: [String: Any]) {
            guard let webView, let data = try? JSONSerialization.data(withJSONObject: value),
                  let string = String(data: data, encoding: .utf8) else { return }
            Task { [weak webView] in
                _ = try? await webView?.callAsyncJavaScript("receiveTeamVisualization(payload)", arguments: ["payload": string], in: nil, contentWorld: .page)
            }
        }
    }
}
