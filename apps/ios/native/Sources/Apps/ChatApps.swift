import Foundation
import Observation
import WebKit

extension ChatApp {
    init(_ dto: LocalAppDto) {
        self.init(
            id: dto.id,
            name: dto.name,
            summary: dto.summary,
            revision: Int(dto.revision),
            updatedAt: Date(timeIntervalSince1970: Double(dto.updatedAtMs) / 1000)
        )
    }
}

/// The engine's app library as state for the pages, and the runtime that runs
/// its apps. The host arrives with the engine (`attach`); until then the list
/// is empty and nothing opens. Every host call blocks on the engine, so each
/// one runs on `queue`.
@MainActor
@Observable
final class ChatApps {
    /// The library, most recently updated first.
    private(set) var apps: [ChatApp] = []
    let runtime = ChatAppRuntime()
    @ObservationIgnored private var host: LocalAppHost?
    nonisolated let queue = DispatchQueue(label: "lingxi.local-apps", qos: .userInitiated)

    /// Use `next` from now on; every engine of this process shares one library.
    func attach(_ next: LocalAppHost) {
        guard next !== host else { return }
        host = next
        runtime.host = next
        Task { await refresh() }
    }

    func refresh() async {
        guard let host else { return }
        let list = await onQueue { host.list() }
        apps = list.map(ChatApp.init)
    }

    /// True or false once the library answered; nil without an engine.
    func exists(_ id: String) async -> Bool? {
        guard let host else { return nil }
        return await onQueue { host.get(id: id) != nil }
    }

    /// Deletes the app, then its browser data; false when the library could
    /// not. The data stays with an app that is still there.
    @discardableResult
    func delete(_ id: String) async -> Bool {
        guard let host else { return false }
        await runtime.close(id)
        let deleted = await onQueue { host.delete(id: id) }
        if deleted { await runtime.clear(id) }
        await refresh()
        return deleted
    }

    private func onQueue<T: Sendable>(_ work: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { continuation in
            queue.async { continuation.resume(returning: work()) }
        }
    }
}

/// Where a chat app's browser data lives: its own `WKWebsiteDataStore`, and the
/// loopback port that makes its origin. Both stay fixed for the app's life, so
/// its storage survives new revisions and relaunches.
struct ChatAppRecord: Codable, Equatable, Sendable {
    let storeID: UUID
    var port: UInt16?
}

/// Every chat app's `ChatAppRecord`, kept in one JSON file in Application Support.
@MainActor
final class ChatAppRegistry {
    private let file: URL?
    private(set) var records: [String: ChatAppRecord] = [:]

    init(file: URL? = ChatAppRegistry.defaultFile()) {
        self.file = file
        if let file, let data = try? Data(contentsOf: file),
           let stored = try? JSONDecoder().decode([String: ChatAppRecord].self, from: data) {
            records = stored
        }
    }

    nonisolated static func defaultFile() -> URL? {
        try? FileManager.default
            .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("chat-apps.json")
    }

    /// The app's record, made and kept on first use.
    func record(_ id: String) throws -> ChatAppRecord {
        if let record = records[id] { return record }
        let record = ChatAppRecord(storeID: UUID())
        try persist(id, record)
        return record
    }

    func savePort(_ id: String, port: UInt16) throws {
        guard var record = records[id], record.port != port else { return }
        record.port = port
        try persist(id, record)
    }

    func forget(_ id: String) {
        var next = records
        guard next.removeValue(forKey: id) != nil else { return }
        try? write(next)
    }

    private func persist(_ id: String, _ record: ChatAppRecord) throws {
        var next = records
        next[id] = record
        try write(next)
    }

    private func write(_ next: [String: ChatAppRecord]) throws {
        if let file { try JSONEncoder().encode(next).write(to: file, options: .atomic) }
        records = next
    }
}

/// Runs chat apps the way `MarketRuntime` runs market apps: each app on its own
/// loopback port in its own `WKWebsiteDataStore`, with no native bridge. Every
/// request is answered by the engine's `LocalAppHost.serve`, which owns the
/// document and its policies.
@MainActor
final class ChatAppRuntime: NSObject, WKNavigationDelegate, WKUIDelegate {
    var host: LocalAppHost?
    var onExternalNavigation: ((URL) -> Void)?
    private let registry: ChatAppRegistry
    private var servers: [String: MarketHTTPServer] = [:]
    private(set) var webViews: [String: WKWebView] = [:]
    private var opening = Set<String>()
    private var closing: [String: Task<Void, Never>] = [:]
    private var generations: [String: UUID] = [:]

    init(registry: ChatAppRegistry = ChatAppRegistry()) { self.registry = registry }

    func open(_ id: String) async throws -> WKWebView {
        if let task = closing[id] { await task.value }
        if let view = webViews[id] { return view }
        guard let host else { throw MarketFailure("ENGINE_UNAVAILABLE") }
        guard opening.insert(id).inserted else { throw MarketFailure("OPEN_IN_PROGRESS") }
        defer { opening.remove(id) }
        let generation = generations[id]
        let record = try registry.record(id)
        let server = MarketHTTPServer { path in
            let response = host.serve(id: id, path: path)
            var headers: [String: String] = [:]
            for header in response.headers { headers[header.name] = header.value }
            return MarketHTTPReply(status: Int(response.status), headers: headers, body: response.body)
        }
        do {
            let port = try await server.start(port: record.port)
            try registry.savePort(id, port: port)
            // Closed or deleted while the server started.
            guard generations[id] == generation else { throw MarketFailure("APP_CHANGED") }
            let configuration = WKWebViewConfiguration()
            configuration.websiteDataStore = WKWebsiteDataStore(forIdentifier: record.storeID)
            configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
            configuration.allowsInlineMediaPlayback = true
            let view = WKWebView(frame: .zero, configuration: configuration)
            view.navigationDelegate = self; view.uiDelegate = self
            servers[id] = server; webViews[id] = view
            view.load(URLRequest(url: URL(string: "http://127.0.0.1:\(port)/")!))
            return view
        } catch { await server.close(); throw error }
    }

    func close(_ id: String) async {
        if let task = closing[id] { await task.value; return }
        generations[id] = UUID()
        if let view = webViews.removeValue(forKey: id) {
            view.stopLoading(); view.navigationDelegate = nil; view.uiDelegate = nil; view.loadHTMLString("", baseURL: nil); view.removeFromSuperview()
        }
        if let server = servers.removeValue(forKey: id) {
            let task = Task { await server.close() }
            closing[id] = task
            await task.value
            closing.removeValue(forKey: id)
        }
    }

    func suspend(_ id: String) { webViews[id]?.stopLoading(); webViews[id]?.setAllMediaPlaybackSuspended(true, completionHandler: nil) }
    func resume(_ id: String) { webViews[id]?.setAllMediaPlaybackSuspended(false, completionHandler: nil) }

    /// Removes the app's browser data and its data store. Close the app first:
    /// a store a web view still uses cannot be removed.
    func clear(_ id: String) async {
        guard let record = registry.records[id] else { return }
        let store = WKWebsiteDataStore(forIdentifier: record.storeID)
        await withCheckedContinuation { continuation in
            store.removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast) { continuation.resume() }
        }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            WKWebsiteDataStore.remove(forIdentifier: record.storeID) { _ in continuation.resume() }
        }
        registry.forget(id)
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
