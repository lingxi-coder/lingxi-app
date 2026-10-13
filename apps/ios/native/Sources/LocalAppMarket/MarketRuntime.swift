import Foundation
import Network
import WebKit
import UniformTypeIdentifiers

/// Static server binds loopback only. No application or native bridge endpoints exist.
final class MarketHTTPServer: @unchecked Sendable {
    private final class CompletionGate: @unchecked Sendable { var completed = false }
    static let csp = "sandbox allow-scripts allow-forms allow-modals allow-downloads allow-same-origin; default-src 'self' data: blob: https:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: https:; style-src 'self' 'unsafe-inline' https:; connect-src 'self' https: wss:; worker-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
    private let root: URL
    private let queue = DispatchQueue(label: "lingxi.market.http")
    private var listener: NWListener?
    private var connections: [UUID: NWConnection] = [:]
    init(root: URL) { self.root = root }
    deinit {
        listener?.cancel()
        for connection in connections.values { connection.cancel() }
    }
    func start(port: UInt16?) async throws -> UInt16 {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: NWEndpoint.Port(rawValue: port ?? 0)!)
        // SO_REUSEADDR permits reopening our stable port after TCP TIME_WAIT;
        // a second live listener still fails with EADDRINUSE.
        parameters.allowLocalEndpointReuse = true
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.newConnectionHandler = { [weak self] in self?.accept($0) }
        return try await withCheckedThrowingContinuation { continuation in
            let gate = CompletionGate()
            listener.stateUpdateHandler = { [weak listener] state in
                guard !gate.completed else { return }
                switch state {
                case .ready:
                    gate.completed = true
                    if let port = listener?.port { continuation.resume(returning: port.rawValue) }
                    else { continuation.resume(throwing: MarketFailure("PORT_UNAVAILABLE")) }
                case .failed(let error): gate.completed = true; continuation.resume(throwing: error)
                case .cancelled: gate.completed = true; continuation.resume(throwing: MarketFailure("SERVER_CANCELLED"))
                default: break
                }
            }
            listener.start(queue: queue)
        }
    }
    func close() async {
        await withCheckedContinuation { continuation in
            queue.async {
                for connection in self.connections.values { connection.cancel() }
                self.connections.removeAll()
                guard let listener = self.listener else { continuation.resume(); return }
                // Cancellation is asynchronous: wait for socket teardown before
                // permitting the stable origin's port to be opened again.
                listener.stateUpdateHandler = { [weak self] state in
                    if case .cancelled = state {
                        self?.listener = nil
                        continuation.resume()
                    }
                }
                listener.cancel()
            }
        }
    }
    private func accept(_ connection: NWConnection) {
        let id = UUID(); connections[id] = connection
        connection.start(queue: queue)
        receive(connection, id: id, buffer: Data())
    }
    private func receive(_ connection: NWConnection, id: UUID, buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { [weak self] data, _, done, error in
            guard let self else { connection.cancel(); return }
            let next = buffer + (data ?? Data())
            if next.count > 16384 { self.respond(connection, id: id, status: 431, body: Data()); return }
            if next.range(of: Data("\r\n\r\n".utf8)) != nil {
                self.serve(connection, id: id, request: String(decoding: next, as: UTF8.self))
            } else if done || error != nil { connection.cancel(); self.connections.removeValue(forKey: id) }
            else { self.receive(connection, id: id, buffer: next) }
        }
    }
    private func serve(_ connection: NWConnection, id: UUID, request: String) {
        let lines = request.components(separatedBy: "\r\n")
        let words = (lines.first ?? "").split(separator: " ")
        guard words.count == 3, words[0] == "GET" || words[0] == "HEAD" else { respond(connection, id: id, status: 405, body: Data()); return }
        let head = words[0] == "HEAD"
        let raw = String(words[1]).components(separatedBy: "?")[0]
        guard raw.hasPrefix("/"), let decoded = raw.removingPercentEncoding else { respond(connection, id: id, status: 400, body: Data()); return }
        let path = decoded == "/" ? "index.html" : String(decoded.dropFirst())
        do {
            let safe = try MarketRepository.safePath(path)
            let target = root.appendingPathComponent(safe).resolvingSymlinksInPath()
            guard target.path.hasPrefix(root.resolvingSymlinksInPath().path + "/") else { throw MarketFailure("UNSAFE_PATH") }
            let attributes = try FileManager.default.attributesOfItem(atPath: target.path)
            guard attributes[.type] as? FileAttributeType == .typeRegular else { throw MarketFailure("NOT_FILE") }
            let data = try Data(contentsOf: target, options: .mappedIfSafe)
            var headers = ["Content-Type": Self.mime(target.pathExtension), "Accept-Ranges": "bytes"]
            if let range = lines.first(where: { $0.lowercased().hasPrefix("range:") }) {
                guard let bounds = Self.range(range.components(separatedBy: ":").dropFirst().joined(separator: ":").trimmingCharacters(in: .whitespaces), size: data.count) else {
                    respond(connection, id: id, status: 416, body: Data(), headers: ["Content-Range": "bytes */\(data.count)"], head: head); return
                }
                headers["Content-Range"] = "bytes \(bounds.lowerBound)-\(bounds.upperBound)/\(data.count)"
                respond(connection, id: id, status: 206, body: data.subdata(in: bounds.lowerBound..<(bounds.upperBound + 1)), headers: headers, head: head)
            } else { respond(connection, id: id, status: 200, body: data, headers: headers, head: head) }
        } catch { respond(connection, id: id, status: 404, body: Data(), head: head) }
    }
    static func range(_ value: String, size: Int) -> ClosedRange<Int>? {
        guard size > 0, value.hasPrefix("bytes="), !value.contains(",") else { return nil }
        let parts = value.dropFirst(6).split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 2 else { return nil }
        if parts[0].isEmpty {
            guard let suffix = Int(parts[1]), suffix > 0 else { return nil }
            return max(0, size - suffix)...(size - 1)
        }
        guard let start = Int(parts[0]), start >= 0, start < size else { return nil }
        let end: Int
        if parts[1].isEmpty { end = size - 1 }
        else { guard let parsed = Int(parts[1]), parsed >= start else { return nil }; end = min(parsed, size - 1) }
        return start...end
    }
    static func mime(_ ext: String) -> String {
        switch ext.lowercased() {
        case "html", "htm": return "text/html; charset=utf-8"
        case "js", "mjs": return "text/javascript; charset=utf-8"
        case "css": return "text/css; charset=utf-8"
        case "json": return "application/json"
        case "wasm": return "application/wasm"
        case "svg": return "image/svg+xml"
        default: return UTType(filenameExtension: ext)?.preferredMIMEType ?? "application/octet-stream"
        }
    }
    private func respond(_ connection: NWConnection, id: UUID, status: Int, body: Data, headers: [String: String] = [:], head: Bool = false) {
        var headers = headers
        headers["Content-Length"] = String(body.count)
        headers["Connection"] = "close"
        headers["Content-Security-Policy"] = Self.csp
        headers["X-Content-Type-Options"] = "nosniff"
        headers["Permissions-Policy"] = "camera=(), microphone=(), geolocation=(), clipboard-read=(), clipboard-write=(), display-capture=()"
        headers["Cache-Control"] = "no-store"
        let reasons = [200: "OK", 206: "Partial Content", 400: "Bad Request", 404: "Not Found", 405: "Method Not Allowed", 416: "Range Not Satisfiable", 431: "Request Header Fields Too Large"]
        var response = Data("HTTP/1.1 \(status) \(reasons[status] ?? "Error")\r\n".utf8)
        for (key, value) in headers { response.append(Data("\(key): \(value)\r\n".utf8)) }
        response.append(Data("\r\n".utf8))
        if !head { response.append(body) }
        connection.send(content: response, completion: .contentProcessed { [weak self] _ in
            connection.cancel(); self?.connections.removeValue(forKey: id)
        })
    }
}

@MainActor
final class MarketRuntime: NSObject, WKNavigationDelegate, WKUIDelegate {
    private let repository: MarketRepository
    private var servers: [String: MarketHTTPServer] = [:]
    private(set) var webViews: [String: WKWebView] = [:]
    private var opening = Set<String>()
    private var closing: [String: Task<Void, Never>] = [:]
    private var generations: [String: UUID] = [:]
    var onExternalNavigation: ((URL) -> Void)?
    init(repository: MarketRepository) { self.repository = repository }
    func open(_ appId: String) async throws -> WKWebView {
        if let task = closing[appId] { await task.value }
        let generation = generations[appId]
        let record = try await repository.installation(appId)
        if let view = webViews[appId] { return view }
        guard opening.insert(appId).inserted else { throw MarketFailure("OPEN_IN_PROGRESS") }
        defer { opening.remove(appId) }
        let server = MarketHTTPServer(root: await repository.resources(record))
        do {
            let port = try await server.start(port: record.port)
            try await repository.savePort(appId, installationID: record.installationID, port: port)
            let current = try await repository.installation(appId)
            guard generations[appId] == generation, current.installationID == record.installationID, current.directory == record.directory else { throw MarketFailure("INSTALLATION_CHANGED") }
            let configuration = WKWebViewConfiguration()
            configuration.websiteDataStore = WKWebsiteDataStore(forIdentifier: record.installationID)
            configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
            configuration.allowsInlineMediaPlayback = true
            let view = WKWebView(frame: .zero, configuration: configuration)
            view.navigationDelegate = self; view.uiDelegate = self
            servers[appId] = server; webViews[appId] = view
            view.load(URLRequest(url: URL(string: "http://127.0.0.1:\(port)/index.html")!))
            return view
        } catch { await server.close(); throw error }
    }
    func close(_ appId: String) async {
        if let task = closing[appId] { await task.value; return }
        generations[appId] = UUID()
        if let view = webViews.removeValue(forKey: appId) {
            view.stopLoading(); view.navigationDelegate = nil; view.uiDelegate = nil; view.loadHTMLString("", baseURL: nil); view.removeFromSuperview()
        }
        if let server = servers.removeValue(forKey: appId) {
            let task = Task { await server.close() }
            closing[appId] = task
            await task.value
            closing.removeValue(forKey: appId)
        }
    }
    func suspend(_ appId: String) { webViews[appId]?.stopLoading(); webViews[appId]?.setAllMediaPlaybackSuspended(true, completionHandler: nil) }
    func resume(_ appId: String) { webViews[appId]?.setAllMediaPlaybackSuspended(false, completionHandler: nil) }
    private func clearBrowserData(_ identifier: UUID) async {
        let store = WKWebsiteDataStore(forIdentifier: identifier)
        await withCheckedContinuation { continuation in
            store.removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast) { continuation.resume() }
        }
    }
    func clear(_ record: MarketInstallation) async throws {
        await close(record.appId)
        await clearBrowserData(record.installationID)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            WKWebsiteDataStore.remove(forIdentifier: record.installationID) { error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
        }
    }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url else { decisionHandler(.cancel); return }
        if url.scheme == "about" { decisionHandler(.allow); return }
        let local = webView.url ?? action.request.mainDocumentURL
        if url.scheme == "http", url.host == "127.0.0.1", url.port == local?.port { decisionHandler(.allow); return }
        if action.targetFrame?.isMainFrame == false, url.scheme == "https" { decisionHandler(.allow); return }
        decisionHandler(.cancel)
        if ["https", "mailto", "tel"].contains(url.scheme ?? "") { onExternalNavigation?(url) }
    }
    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin, initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType, decisionHandler: @escaping (WKPermissionDecision) -> Void) { decisionHandler(.deny) }
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = action.request.url, ["https", "mailto", "tel"].contains(url.scheme ?? "") { onExternalNavigation?(url) }
        return nil
    }
}
