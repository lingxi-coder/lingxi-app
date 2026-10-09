import SwiftUI
import WebKit

/// Inline visualizations on iOS.
///
/// Each widget renders in its own `WKWebView` on a non-persistent data store.
/// The main frame is the engine's trusted shell served from
/// `lingxi-viz://visualization`; the author fragment runs one frame deeper in
/// an opaque-origin sandbox the shell creates. Every byte comes from the
/// engine through `VisualizationHost.serve` — authorization, CSP headers and
/// state compare-and-swap all live there, identical to the desktop host. A
/// content rule list blocks every load outside the scheme, so a widget cannot
/// reach the network even if a CSP were bypassed.
@MainActor
final class VisualizationWebHost {
    static let shared = VisualizationWebHost()
    static let scheme = "lingxi-viz"
    static let origin = "lingxi-viz://visualization"
    static let shellURL = URL(string: "lingxi-viz://visualization/shell.html")!
    static let messageHandler = "lingxiVisualization"
    static let maxInlineHeight: CGFloat = 640
    static let minHeight: CGFloat = 32

    /// The engine's host; `nil` until an engine handle exists.
    private(set) var host: VisualizationHost?
    /// Blocking engine calls never run on the main thread.
    nonisolated let queue = DispatchQueue(label: "lingxi.visualization", qos: .userInitiated)
    private var ruleList: WKContentRuleList?
    private var ruleListWaiters: [(WKContentRuleList?) -> Void] = []
    private var compilingRules = false

    func attach(_ host: VisualizationHost?) {
        self.host = host
    }

    /// Block every load that is not this scheme (network, file, data URLs
    /// to other origins). Compiled once and shared by every widget.
    func withRuleList(_ completion: @escaping (WKContentRuleList?) -> Void) {
        if let ruleList { return completion(ruleList) }
        ruleListWaiters.append(completion)
        guard !compilingRules else { return }
        compilingRules = true
        let rules = """
        [{"trigger":{"url-filter":".*"},"action":{"type":"block"}},
         {"trigger":{"url-filter":"^lingxi-viz://visualization/"},"action":{"type":"ignore-previous-rules"}}]
        """
        WKContentRuleListStore.default().compileContentRuleList(
            forIdentifier: "lingxi-visualization-v1",
            encodedContentRuleList: rules
        ) { [weak self] list, _ in
            Task { @MainActor in
                guard let self else { return }
                self.ruleList = list
                self.compilingRules = false
                let waiters = self.ruleListWaiters
                self.ruleListWaiters = []
                waiters.forEach { $0(list) }
            }
        }
    }
}

/// Answers `lingxi-viz://visualization/...` from the engine.
final class VisualizationSchemeHandler: NSObject, WKURLSchemeHandler {
    private let host: VisualizationHost
    private let queue: DispatchQueue
    private var stopped = Set<ObjectIdentifier>()
    private let lock = NSLock()

    init(host: VisualizationHost, queue: DispatchQueue) {
        self.host = host
        self.queue = queue
    }

    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        guard let url = urlSchemeTask.request.url,
              url.scheme == VisualizationWebHost.scheme,
              url.host == "visualization",
              url.user == nil, url.password == nil, url.port == nil,
              url.query == nil, url.fragment == nil
        else {
            urlSchemeTask.didFailWithError(URLError(.unsupportedURL))
            return
        }
        let path = url.path
        let key = ObjectIdentifier(urlSchemeTask)
        queue.async { [host, weak self] in
            let response = host.serve(path: path)
            DispatchQueue.main.async {
                guard let self else { return }
                self.lock.lock()
                let wasStopped = self.stopped.remove(key) != nil
                self.lock.unlock()
                if wasStopped { return }
                var headers: [String: String] = [:]
                for header in response.headers { headers[header.name] = header.value }
                guard let http = HTTPURLResponse(
                    url: url,
                    statusCode: Int(response.status),
                    httpVersion: "HTTP/1.1",
                    headerFields: headers
                ) else {
                    urlSchemeTask.didFailWithError(URLError(.badServerResponse))
                    return
                }
                urlSchemeTask.didReceive(http)
                urlSchemeTask.didReceive(response.body)
                urlSchemeTask.didFinish()
            }
        }
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
        lock.lock()
        stopped.insert(ObjectIdentifier(urlSchemeTask))
        lock.unlock()
    }
}

/// Breaks the retain cycle `WKUserContentController` holds on its handler.
private final class WeakScriptHandler: NSObject, WKScriptMessageHandler {
    weak var target: WKScriptMessageHandler?
    init(_ target: WKScriptMessageHandler) { self.target = target }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(controller, didReceive: message)
    }
}

/// One mounted widget: owns its web view, mount token and shell relay.
@MainActor
final class VisualizationWidgetController: NSObject, ObservableObject, WKScriptMessageHandler, WKNavigationDelegate, WKUIDelegate {
    enum Phase: Equatable { case loading, ready, unavailable, crashed }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var height: CGFloat = 160
    @Published private(set) var title: String = ""

    let sessionId: String
    let id: String
    let revision: UInt32
    var dark: Bool
    var onFollowup: ((VisualizationFollowup) -> Void)?
    private(set) var webView: WKWebView?
    private var mount: VisualizationMountDto?
    private static let maxMessageCharacters = 64 * 1024

    init(sessionId: String, id: String, revision: UInt32, dark: Bool) {
        self.sessionId = sessionId
        self.id = id
        self.revision = revision
        self.dark = dark
        super.init()
    }

    func makeWebView(_ completion: @escaping (WKWebView?) -> Void) {
        if let webView { return completion(webView) }
        guard let host = VisualizationWebHost.shared.host else {
            phase = .unavailable
            return completion(nil)
        }
        VisualizationWebHost.shared.withRuleList { [weak self] rules in
            guard let self else { return completion(nil) }
            // Without the network block the widget does not load at all.
            guard let rules else {
                self.phase = .unavailable
                return completion(nil)
            }
            let configuration = WKWebViewConfiguration()
            configuration.websiteDataStore = .nonPersistent()
            configuration.setURLSchemeHandler(
                VisualizationSchemeHandler(host: host, queue: VisualizationWebHost.shared.queue),
                forURLScheme: VisualizationWebHost.scheme
            )
            configuration.userContentController.add(rules)
            configuration.userContentController.add(WeakScriptHandler(self), contentWorld: .page, name: VisualizationWebHost.messageHandler)
            configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
            configuration.defaultWebpagePreferences.allowsContentJavaScript = true
            configuration.allowsInlineMediaPlayback = true
            configuration.mediaTypesRequiringUserActionForPlayback = .all
            configuration.dataDetectorTypes = []
            let view = WKWebView(frame: .zero, configuration: configuration)
            view.isOpaque = false
            view.backgroundColor = .clear
            view.scrollView.isScrollEnabled = false
            view.scrollView.bounces = false
            view.allowsLinkPreview = false
            view.allowsBackForwardNavigationGestures = false
            view.navigationDelegate = self
            view.uiDelegate = self
            #if DEBUG
            if #available(iOS 16.4, *) { view.isInspectable = true }
            #endif
            self.webView = view
            view.load(URLRequest(url: VisualizationWebHost.shellURL))
            completion(view)
        }
    }

    /// Retire the mount and drop the web view (scrolled far away, or gone).
    func suspend() {
        retire()
        webView?.navigationDelegate = nil
        webView?.configuration.userContentController.removeScriptMessageHandler(
            forName: VisualizationWebHost.messageHandler, contentWorld: .page
        )
        webView?.removeFromSuperview()
        webView = nil
        phase = .loading
    }

    private func retire() {
        guard let mount else { return }
        self.mount = nil
        let token = mount.token
        let host = VisualizationWebHost.shared.host
        VisualizationWebHost.shared.queue.async { host?.unmount(token: token) }
    }

    private func post(_ message: [String: Any]) {
        guard let webView,
              let data = try? JSONSerialization.data(withJSONObject: message),
              let raw = String(data: data, encoding: .utf8),
              let literalData = try? JSONSerialization.data(withJSONObject: [raw]),
              let literal = String(data: literalData, encoding: .utf8)
        else { return }
        // `[raw]` encodes the string as a safe JS literal; index 0 unwraps it.
        webView.evaluateJavaScript("window.__lingxiVisualizationDeliver(\(literal)[0])", in: nil, in: .page)
    }

    // MARK: shell → host

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame,
              message.frameInfo.securityOrigin.protocol == VisualizationWebHost.scheme,
              message.frameInfo.securityOrigin.host == "visualization",
              let raw = message.body as? String,
              raw.count <= Self.maxMessageCharacters,
              let data = raw.data(using: .utf8),
              let body = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = body["type"] as? String
        else { return }
        if type == "shell.ready" {
            mountWidget()
            return
        }
        guard let mount, (body["generation"] as? NSNumber)?.uint64Value == mount.generation else { return }
        switch type {
        case "ready":
            phase = .ready
        case "resize":
            if let value = (body["height"] as? NSNumber)?.doubleValue, value.isFinite {
                height = min(VisualizationWebHost.maxInlineHeight, max(VisualizationWebHost.minHeight, ceil(value)))
            }
        case "state.save":
            saveState(body, mount: mount)
        case "followup.draft":
            if let text = body["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                onFollowup?(VisualizationFollowup(
                    text: text,
                    chip: VisualizationContextChip(id: id, revision: revision, title: mount.title)
                ))
            }
        case "crashed":
            phase = .crashed
            remount()
        default:
            break
        }
    }

    private func mountWidget() {
        retire()
        phase = .loading
        guard let host = VisualizationWebHost.shared.host else {
            phase = .unavailable
            return
        }
        let (sessionId, id, revision, dark) = (self.sessionId, self.id, self.revision, self.dark)
        let locale = Locale.preferredLanguages.first ?? "en"
        VisualizationWebHost.shared.queue.async {
            let ticket = host.mount(
                sessionId: sessionId,
                id: id,
                revision: revision,
                themeDto: VisualizationThemeDto(dark: dark, tokens: [:]),
                locale: locale,
                expanded: false
            )
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                guard let ticket else {
                    self.phase = .unavailable
                    return
                }
                self.mount = ticket
                self.title = ticket.title
                self.post([
                    "type": "mount",
                    "generation": ticket.generation,
                    "docUrl": ticket.docUrl,
                    "title": ticket.title,
                    "maxHeight": Double(VisualizationWebHost.maxInlineHeight),
                    "expanded": false,
                ])
            }
        }
    }

    private func saveState(_ body: [String: Any], mount: VisualizationMountDto) {
        guard let requestId = body["requestId"] as? NSNumber,
              let baseVersion = (body["baseVersion"] as? NSNumber)?.uint64Value,
              let modelContent = body["modelContent"] as? String,
              let privateContent = body["privateContent"] as? String,
              let host = VisualizationWebHost.shared.host
        else { return }
        VisualizationWebHost.shared.queue.async {
            let result = host.writeState(
                token: mount.token,
                generation: mount.generation,
                baseVersion: baseVersion,
                modelContentJson: modelContent,
                privateContentJson: privateContent
            )
            DispatchQueue.main.async { [weak self] in
                guard let self, self.mount?.token == mount.token else { return }
                if result.saved {
                    self.post(["type": "state.saved", "generation": mount.generation, "requestId": requestId, "version": result.version])
                } else {
                    let state: Any = result.currentStateJson
                        .flatMap { $0.data(using: .utf8) }
                        .flatMap { try? JSONSerialization.jsonObject(with: $0) } ?? NSNull()
                    self.post([
                        "type": "state.rejected", "generation": mount.generation, "requestId": requestId,
                        "reason": result.reason ?? "rejected", "state": state,
                    ])
                }
            }
        }
    }

    /// Reload the shell; it asks for a fresh mount (documents are single use).
    func remount() {
        retire()
        webView?.load(URLRequest(url: VisualizationWebHost.shellURL))
    }

    func setDark(_ dark: Bool) {
        guard dark != self.dark else { return }
        self.dark = dark
        if webView != nil { remount() }
    }

    // MARK: navigation

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        let url = navigationAction.request.url
        let allowed = url?.scheme == VisualizationWebHost.scheme && url?.host == "visualization"
            || url?.absoluteString == "about:blank" || url?.absoluteString == "about:srcdoc"
        decisionHandler(allowed && navigationAction.navigationType != .linkActivated ? .allow : .cancel)
    }

    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        nil
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        phase = .crashed
        remount()
    }
}

/// Hosts the controller's web view inside SwiftUI.
private struct VisualizationWebViewRepresentable: UIViewRepresentable {
    let controller: VisualizationWidgetController

    func makeUIView(context: Context) -> UIView {
        let container = UIView()
        container.backgroundColor = .clear
        controller.makeWebView { view in
            guard let view else { return }
            view.translatesAutoresizingMaskIntoConstraints = false
            container.addSubview(view)
            NSLayoutConstraint.activate([
                view.leadingAnchor.constraint(equalTo: container.leadingAnchor),
                view.trailingAnchor.constraint(equalTo: container.trailingAnchor),
                view.topAnchor.constraint(equalTo: container.topAnchor),
                view.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            ])
        }
        return container
    }

    func updateUIView(_ uiView: UIView, context: Context) {}

    static func dismantleUIView(_ uiView: UIView, coordinator: ()) {
        uiView.subviews.forEach { $0.removeFromSuperview() }
    }
}

/// One inline widget in the transcript.
struct VisualizationCardView: View {
    let sessionId: String
    let visualization: MessageVisualization
    let onFollowup: (VisualizationFollowup) -> Void

    var body: some View {
        switch visualization.status {
        case .pending:
            VisualizationNoteView(text: String(localized: "visualization_preparing"), busy: true)
        case .unavailable:
            VisualizationNoteView(text: String(localized: "visualization_unavailable"), busy: false)
        case .ready:
            if let id = visualization.id, let revision = visualization.revision, !sessionId.isEmpty {
                VisualizationLiveCard(sessionId: sessionId, id: id, revision: revision, onFollowup: onFollowup)
                    .id("\(sessionId)|\(id)|\(revision)")
            } else {
                VisualizationNoteView(text: String(localized: "visualization_unavailable"), busy: false)
            }
        }
    }
}

private struct VisualizationLiveCard: View {
    @Environment(\.colorScheme) private var colorScheme
    @StateObject private var controller: VisualizationWidgetController
    let onFollowup: (VisualizationFollowup) -> Void

    init(sessionId: String, id: String, revision: UInt32, onFollowup: @escaping (VisualizationFollowup) -> Void) {
        _controller = StateObject(wrappedValue: VisualizationWidgetController(
            sessionId: sessionId, id: id, revision: revision, dark: false
        ))
        self.onFollowup = onFollowup
    }

    var body: some View {
        ZStack {
            if controller.phase == .unavailable {
                VisualizationNoteView(text: String(localized: "visualization_unavailable"), busy: false)
            } else {
                VisualizationWebViewRepresentable(controller: controller)
                    .frame(height: controller.height)
                    .accessibilityLabel(controller.title.isEmpty
                        ? String(localized: "visualization_label")
                        : controller.title)
                if controller.phase != .ready {
                    VisualizationNoteView(text: String(localized: "visualization_loading"), busy: true)
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .onAppear {
            controller.dark = colorScheme == .dark
            controller.onFollowup = onFollowup
        }
        .onChange(of: colorScheme) { _, scheme in controller.setDark(scheme == .dark) }
        .onDisappear { controller.suspend() }
    }
}

private struct VisualizationNoteView: View {
    let text: String
    let busy: Bool

    var body: some View {
        HStack(spacing: 8) {
            if busy { ProgressView().controlSize(.small) }
            Text(text).font(.footnote).foregroundStyle(.secondary)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, minHeight: busy ? 96 : nil, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color(.secondarySystemBackground)))
    }
}

/// The widget a user message (or the composer draft) follows up on.
struct VisualizationContextChipView: View {
    let chip: VisualizationContextChip
    var onDismiss: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "chart.bar.xaxis")
                .font(.caption)
            Text(chip.title.isEmpty ? String(localized: "visualization_label") : chip.title)
                .font(.caption)
                .lineLimit(1)
            if let onDismiss {
                Button(action: onDismiss) {
                    Image(systemName: "xmark").font(.caption2)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "visualization_remove_context"))
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .foregroundStyle(.secondary)
        .background(Capsule().fill(Color(.secondarySystemBackground)))
        .accessibilityElement(children: .combine)
    }
}
