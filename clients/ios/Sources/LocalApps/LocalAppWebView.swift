import SwiftUI
import WebKit

struct LocalAppBridgeRequest: Identifiable, Sendable {
    let id: String
    let appID: String
    let namespace: String
    let operation: String
    let payloadJSON: String?
}

struct LocalAppUIExecutionResult: Sendable {
    let resultJSON: String?
    let error: String?

    static func failure(_ message: String) -> Self {
        Self(resultJSON: nil, error: message)
    }
}

@MainActor
final class LocalAppWebViewRegistry {
    static let shared = LocalAppWebViewRegistry()

    private final class WeakController {
        weak var value: LocalAppWebViewController?

        init(_ value: LocalAppWebViewController) {
            self.value = value
        }
    }

    private var controllers: [String: WeakController] = [:]

    private init() {}

    func register(_ controller: LocalAppWebViewController, appID: String) {
        // A rebuild re-registers under the same id. The OUTGOING controller
        // is the only one that can still reach its page, so it has to reject
        // its own outstanding requests before it stops being routable —
        // afterwards `resolveBridge` would just drop their answers.
        if let replaced = controllers[appID]?.value, replaced !== controller {
            replaced.close()
        }
        controllers[appID] = WeakController(controller)
    }

    func unregister(_ controller: LocalAppWebViewController, appID: String) {
        if controllers[appID]?.value === controller {
            controllers[appID] = nil
        }
        // Even an outgoing controller that was already replaced still owns
        // its page long enough to reject page promises during dismantling.
        controller.close()
    }

    /// Detaches an app before its persistent website data is removed.
    ///
    /// WebKit requires every view using an identified data store to be released
    /// before `remove(forIdentifier:)` runs. The local-app library normally has
    /// no preview mounted while its delete confirmation is visible, but this
    /// explicit close also covers a controller retained by a transition and
    /// guarantees that page promises do not remain pending forever.
    func close(appID: String) {
        guard let controller = controllers.removeValue(forKey: appID)?.value else { return }
        controller.close()
    }

    func resolveBridge(
        appID: String,
        requestID: String,
        resultJSON: String?,
        error: String?,
        code: String? = nil
    ) {
        guard let controller = controllers[appID]?.value else { return }
        let result = resultJSON.flatMap(Self.decodeJSON)
        let invalidResult = resultJSON != nil && result == nil
        controller.broker.resolve(
            requestID: requestID,
            result: result,
            error: invalidResult ? String(localized: "local_apps_error_bridge_invalid_json") : error,
            code: code
        )
    }

    #if canImport(engine_mobileFFI)
        func execute(request: AppUiRequestDto) async -> LocalAppUIExecutionResult {
            for _ in 0 ..< 50 {
                if let controller = controllers[request.appId]?.value, controller.isReady {
                    return await controller.execute(request: request)
                }
                try? await Task.sleep(for: .milliseconds(200))
            }
            return .failure(String(localized: "local_apps_error_ui_not_open"))
        }
    #endif

    private static func decodeJSON(_ value: String) -> Any? {
        guard let data = value.data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data, options: .fragmentsAllowed)
    }
}

final class LocalAppBridgeBroker: NSObject, WKScriptMessageHandler {
    /// Control operations stay small. Model input gets a separate bounded
    /// lane so a long provider context is not confused with control traffic.
    static let maxControlBytes = 64 * 1_024
    static let maxLLMBytes = 8 * 1_024 * 1_024
    static let maxInFlightRequests = 128
    static let requestIDInvalidCode = "request_id_invalid"
    static let operationInvalidCode = "operation_invalid"
    static let duplicateRequestIDCode = "duplicate_request_id"
    static let tooManyInFlightCode = "too_many_requests"
    static func byteLimit(namespace: String, operation: String) -> Int {
        namespace == "llm" && operation == "chat" ? maxLLMBytes : maxControlBytes
    }

    let appID: String
    var onRequest: ((LocalAppBridgeRequest) -> Void)?

    weak var webView: WKWebView?

    /// Requests handed to the engine that have not been answered yet.
    ///
    /// The page holds a promise per entry and has no timeout of its own —
    /// deliberately, because a capture waits on a user browsing their photo
    /// library and `llm.chat` may take two minutes, so a blanket deadline
    /// would reject legitimate work. Instead the ONE case where an answer
    /// can never arrive — this controller stops being the one the registry
    /// routes to — rejects them explicitly. Without that the page sits on an
    /// `await` that never settles and no `finally` ever runs: a disabled
    /// button stays disabled, a Blob URL is never revoked.
    private var inFlight: Set<String> = []
    private var isDetached = false

    init(appID: String, onRequest: ((LocalAppBridgeRequest) -> Void)? = nil) {
        self.appID = appID
        self.onRequest = onRequest
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame else { return }
        let namespace = message.name.replacingOccurrences(of: "lingxi", with: "").lowercased()
        receive(body: message.body, namespace: namespace)
    }

    /// Internal entry point so admission limits can be regression-tested
    /// without manufacturing a private WebKit `WKScriptMessage` initializer.
    func receive(body rawBody: Any, namespace: String) {
        guard !isDetached else { return }
        guard let body = rawBody as? [String: Any],
              let requestID = body["requestId"] as? String,
              !requestID.isEmpty
        else { return }
        guard requestID.count <= 128 else {
            rejectUntracked(
                requestID: requestID,
                error: String(localized: "local_apps_error_bridge_rejected"),
                code: Self.requestIDInvalidCode)
            return
        }
        guard let operation = body["operation"] as? String,
              !operation.isEmpty,
              operation.count <= 128
        else {
            rejectUntracked(
                requestID: requestID,
                error: String(localized: "local_apps_error_bridge_rejected"),
                code: Self.operationInvalidCode)
            return
        }

        let byteLimit = Self.byteLimit(namespace: namespace, operation: operation)
        if let byteCount = Self.oversizedRequestByteCount(body, limit: byteLimit) {
            rejectUntracked(
                requestID: requestID,
                error: String(
                    localized:
                        "local_apps_error_bridge_payload_too_large \(byteCount) \(byteLimit)"),
                code: "request_too_large")
            return
        }

        let rawPayload: [String: Any]
        if let suppliedPayload = body["payload"] {
            guard let objectPayload = suppliedPayload as? [String: Any] else {
                rejectUntracked(
                    requestID: requestID,
                    error: String(localized: "local_apps_error_bridge_payload_invalid"),
                    code: "payload_invalid")
                return
            }
            rawPayload = objectPayload
        } else {
            rawPayload = [:]
        }
        // A payload that is too large or not serializable used to be replaced
        // by `nil` and forwarded anyway — and the engine reads a missing
        // payload as `{}`, so a SIZE failure came back as a SCHEMA failure
        // ("messages must be an array") pointing the app's author at the
        // wrong field entirely. Refuse it here, with its own code.
        guard JSONSerialization.isValidJSONObject(rawPayload),
              let data = try? JSONSerialization.data(withJSONObject: rawPayload)
        else {
            rejectUntracked(
                requestID: requestID,
                error: String(localized: "local_apps_error_bridge_payload_invalid"),
                code: "payload_invalid")
            return
        }
        guard data.count <= byteLimit else {
            rejectUntracked(
                requestID: requestID,
                error: String(
                    localized:
                        "local_apps_error_bridge_payload_too_large \(data.count) \(byteLimit)"),
                code: "payload_too_large")
            return
        }
        guard !inFlight.contains(requestID) else {
            rejectUntracked(
                requestID: requestID,
                error: String(localized: "local_apps_error_bridge_rejected"),
                code: Self.duplicateRequestIDCode)
            return
        }
        guard inFlight.count < Self.maxInFlightRequests else {
            rejectUntracked(
                requestID: requestID,
                error: String(localized: "local_apps_error_bridge_rejected"),
                code: Self.tooManyInFlightCode)
            return
        }
        let payloadJSON = String(data: data, encoding: .utf8)
        inFlight.insert(requestID)
        onRequest?(
            LocalAppBridgeRequest(
                id: requestID,
                appID: appID,
                namespace: namespace,
                operation: operation,
                payloadJSON: payloadJSON
            )
        )
    }

    static func oversizedRequestByteCount(_ body: [String: Any], limit: Int) -> Int? {
        guard JSONSerialization.isValidJSONObject(body),
              let data = try? JSONSerialization.data(withJSONObject: body),
              data.count > limit
        else { return nil }
        return data.count
    }

    func resolve(requestID: String, result: Any?, error: String?, code: String? = nil) {
        inFlight.remove(requestID)
        sendEnvelope(requestID: requestID, result: result, error: error, code: code)
    }

    var inFlightCount: Int { inFlight.count }

    private func rejectUntracked(requestID: String, error: String, code: String) {
        sendEnvelope(requestID: requestID, result: nil, error: error, code: code)
    }

    /// Sends an answer without changing admission bookkeeping. Validation
    /// failures happen before insertion, and especially a duplicate rejection
    /// must not remove the original request from `inFlight`.
    private func sendEnvelope(requestID: String, result: Any?, error: String?, code: String?) {
        guard let webView else { return }
        let envelope: [String: Any] = [
            "requestId": requestID,
            "result": result ?? NSNull(),
            "error": error ?? NSNull(),
            "code": code ?? NSNull(),
        ]
        guard JSONSerialization.isValidJSONObject(envelope),
              let data = try? JSONSerialization.data(withJSONObject: envelope),
              let json = String(data: data, encoding: .utf8)
        else { return }
        webView.evaluateJavaScript("window.lingxi?.__resolve(\(json));")
    }

    /// Reject everything still awaiting an answer this broker can no longer
    /// deliver. Called when the registry stops routing to this controller.
    func failAllInFlight() {
        let outstanding = inFlight
        inFlight.removeAll()
        for requestID in outstanding {
            resolve(
                requestID: requestID,
                result: nil,
                error: String(localized: "local_apps_error_bridge_detached"),
                code: "bridge_detached")
        }
    }

    /// Permanently stops this broker from accepting messages from a WebView
    /// that is being replaced or deleted. Outstanding promises are rejected
    /// first while the view is still reachable.
    func detach() {
        failAllInFlight()
        isDetached = true
        onRequest = nil
    }
}

@MainActor
final class LocalAppWebViewController {
    let appID: String
    let broker: LocalAppBridgeBroker
    weak var webView: WKWebView?

    /// A controller is routable only after WebKit has committed the page.
    /// `makeUIView` registers the controller before starting navigation so the
    /// engine request is not lost, therefore the registry must distinguish
    /// "mounted" from "ready".
    private(set) var isReady = true

    init(appID: String, broker: LocalAppBridgeBroker) {
        self.appID = appID
        self.broker = broker
    }

    func close() {
        isReady = false
        broker.detach()
        webView?.stopLoading()
        for name in LocalAppWebViewRepresentable.messageHandlerNames {
            webView?.configuration.userContentController.removeScriptMessageHandler(forName: name)
        }
        webView?.navigationDelegate = nil
        webView?.uiDelegate = nil
        broker.webView = nil
        webView = nil
    }

    func markNotReady() {
        isReady = false
    }

    func markReady() {
        isReady = true
    }

    #if canImport(engine_mobileFFI)
        func execute(request: AppUiRequestDto) async -> LocalAppUIExecutionResult {
            guard request.appId == appID else {
                return .failure(String(localized: "local_apps_error_ui_appid_mismatch"))
            }
            guard let webView else {
                return .failure(String(localized: "local_apps_error_ui_closed"))
            }

            if request.action == .back {
                guard webView.canGoBack else { return .failure(String(localized: "local_apps_error_ui_no_history")) }
                markNotReady()
                webView.goBack()
                return encodedResult(["ok": true, "action": "back"])
            }
            if request.action == .reload {
                markNotReady()
                webView.reload()
                return encodedResult(["ok": true, "action": "reload"])
            }

            let payload: [String: Any] = [
                "action": actionName(request.action),
                "target": targetPayload(request.target),
                "value": request.value ?? NSNull(),
            ]
            guard JSONSerialization.isValidJSONObject(payload),
                  let data = try? JSONSerialization.data(withJSONObject: payload),
                  let json = String(data: data, encoding: .utf8)
            else {
                return .failure(String(localized: "local_apps_error_ui_encode_failed"))
            }

            do {
                let rawResult = try await evaluate(Self.executionSource(requestJSON: json), in: webView)
                guard let resultJSON = rawResult as? String,
                      resultJSON.utf8.count <= 256 * 1_024
                else {
                    return .failure(String(localized: "local_apps_error_ui_invalid_result"))
                }
                return LocalAppUIExecutionResult(resultJSON: resultJSON, error: nil)
            } catch {
                // WebKit reports a script throw as a generic "A JavaScript exception
                // occurred"; the diagnostic the agent needs is only in userInfo.
                let info = (error as NSError).userInfo
                let thrown = info["WKJavaScriptExceptionMessage"] as? String
                    ?? info["_WKJavaScriptExceptionMessage"] as? String
                return .failure(thrown ?? error.localizedDescription)
            }
        }

        private func actionName(_ action: AppUiActionKindDto) -> String {
            switch action {
            case .inspect: "inspect"
            case .click: "click"
            case .fill: "fill"
            case .select: "select"
            case .toggle: "toggle"
            case .scroll: "scroll"
            case .navigate: "navigate"
            case .back: "back"
            case .reload: "reload"
            }
        }

        private func targetPayload(_ target: AppUiTargetDto?) -> Any {
            guard let target else { return NSNull() }
            return [
                "elementId": target.elementId ?? NSNull(),
                "role": target.role ?? NSNull(),
                "name": target.name ?? NSNull(),
            ] as [String: Any]
        }

        private func evaluate(_ source: String, in webView: WKWebView) async throws -> Any? {
            try await withCheckedThrowingContinuation { continuation in
                webView.evaluateJavaScript(source) { result, error in
                    if let error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume(returning: result)
                    }
                }
            }
        }
    #endif

    private func encodedResult(_ value: [String: Any]) -> LocalAppUIExecutionResult {
        guard let data = try? JSONSerialization.data(withJSONObject: value),
              let json = String(data: data, encoding: .utf8)
        else { return .failure(String(localized: "local_apps_error_ui_result_encode")) }
        return LocalAppUIExecutionResult(resultJSON: json, error: nil)
    }

    private static func executionSource(requestJSON: String) -> String {
        #"""
        (() => {
          const request = \#(requestJSON);
          const clean = value => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, 500);
          const roleOf = element => clean(element.getAttribute('role') || ({
            BUTTON: 'button', A: 'link', INPUT: element.type === 'checkbox' ? 'checkbox' : (element.type === 'radio' ? 'radio' : 'textbox'),
            SELECT: 'combobox', TEXTAREA: 'textbox'
          })[element.tagName] || '');
          const nameOf = element => {
            const labelledBy = element.getAttribute('aria-labelledby');
            const labelled = labelledBy ? document.getElementById(labelledBy) : null;
            const explicit = element.id ? document.querySelector(`label[for="${CSS.escape(element.id)}"]`) : null;
            return clean(element.getAttribute('aria-label') || labelled?.textContent || explicit?.textContent || element.placeholder || element.innerText || element.value);
          };
          const candidates = () => Array.from(document.querySelectorAll('button,a[href],input,select,textarea,[role],[tabindex],[contenteditable="true"]'));
          const findTarget = target => {
            if (!target) return null;
            if (target.elementId) {
              const byId = document.getElementById(target.elementId);
              if (byId) return byId;
            }
            return candidates().find(element =>
              (!target.role || roleOf(element).toLowerCase() === clean(target.role).toLowerCase()) &&
              (!target.name || nameOf(element).toLowerCase() === clean(target.name).toLowerCase())
            ) || null;
          };
          const dispatchValueChange = element => {
            element.dispatchEvent(new Event('input', { bubbles: true }));
            element.dispatchEvent(new Event('change', { bubbles: true }));
          };
          const setNativeValue = (element, value) => {
            const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
            const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
            if (!setter) throw new Error('Target value cannot be changed');
            setter.call(element, value);
          };
          const snapshot = () => ({
            title: clean(document.title),
            url: location.href,
            elements: candidates().slice(0, 200).map(element => {
              const rect = element.getBoundingClientRect();
              const sensitive = element instanceof HTMLInputElement && ['hidden', 'password'].includes(element.type);
              return {
                elementId: clean(element.id) || null,
                role: roleOf(element) || null,
                name: nameOf(element) || null,
                value: sensitive ? null : clean(element.value),
                checked: typeof element.checked === 'boolean' ? element.checked : null,
                disabled: !!element.disabled,
                visible: rect.width > 0 && rect.height > 0,
              };
            }),
          });
          try {
            if (request.action === 'inspect') return JSON.stringify(snapshot());
            if (request.action === 'navigate') {
              const destination = new URL(request.value || '', location.href);
              if (destination.origin !== location.origin) throw new Error('Only same-origin navigation is allowed');
              location.assign(destination.href);
              return JSON.stringify({ ok: true, action: 'navigate', url: destination.href });
            }
            if (request.action === 'scroll') {
              const target = findTarget(request.target);
              if (target) {
                target.scrollIntoView({ behavior: 'smooth', block: 'center' });
              } else {
                const raw = clean(request.value).toLowerCase();
                const delta = raw === 'up' ? -window.innerHeight * 0.8 : raw === 'down' ? window.innerHeight * 0.8 : raw === 'top' ? -document.body.scrollHeight : raw === 'bottom' ? document.body.scrollHeight : Number(raw || window.innerHeight * 0.8);
                if (!Number.isFinite(delta)) throw new Error('Invalid scroll value');
                window.scrollBy({ top: delta, behavior: 'smooth' });
              }
              return JSON.stringify({ ok: true, action: 'scroll' });
            }
            const element = findTarget(request.target);
            if (!element) throw new Error('UI target was not found');
            if (element.disabled) throw new Error('UI target is disabled');
            if (request.action === 'click') {
              element.click();
            } else if (request.action === 'fill') {
              if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element.isContentEditable)) throw new Error('Target cannot be filled');
              if (element.isContentEditable) element.textContent = request.value || '';
              else setNativeValue(element, request.value || '');
              dispatchValueChange(element);
            } else if (request.action === 'select') {
              if (!(element instanceof HTMLSelectElement)) throw new Error('Target is not a select element');
              const option = Array.from(element.options).find(item => item.value === request.value || clean(item.textContent) === clean(request.value));
              if (!option) throw new Error('Select option was not found');
              setNativeValue(element, option.value);
              dispatchValueChange(element);
            } else if (request.action === 'toggle') {
              // A supplied value is the REQUESTED state, so an element already in it must not flip.
              const desired = clean(request.value).toLowerCase();
              const current = element instanceof HTMLInputElement && (element.type === 'checkbox' || element.type === 'radio')
                ? !!element.checked
                : ['checkbox', 'switch'].includes(roleOf(element).toLowerCase())
                  ? clean(element.getAttribute('aria-checked')).toLowerCase() === 'true'
                  : null;
              if (current === null) throw new Error('Target is not toggleable');
              if (desired === '' || current !== (desired === 'true')) element.click();
            } else {
              throw new Error('Unsupported UI action');
            }
            return JSON.stringify({ ok: true, action: request.action, target: { elementId: element.id || null, role: roleOf(element) || null, name: nameOf(element) || null } });
          } catch (error) {
            throw new Error(`Lingxi UI action failed: ${error.message}`);
          }
        })();
        """#
    }
}

struct LocalAppWebView: View {
    let appID: String
    let url: URL
    var onBridgeRequest: ((LocalAppBridgeRequest) -> Void)? = nil

    @State private var pendingExternalURL: URL?

    var body: some View {
        LocalAppWebViewRepresentable(
            appID: appID,
            url: url,
            onBridgeRequest: onBridgeRequest,
            onExternalNavigation: { pendingExternalURL = $0 }
        )
        .ignoresSafeArea(.container, edges: .bottom)
        .confirmationDialog(
            "local_apps_external_link_title",
            isPresented: Binding(
                get: { pendingExternalURL != nil },
                set: { if !$0 { pendingExternalURL = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("common_open") {
                guard let pendingExternalURL else { return }
                UIApplication.shared.open(pendingExternalURL)
                self.pendingExternalURL = nil
            }
            Button("common_cancel", role: .cancel) { pendingExternalURL = nil }
        } message: {
            Text(pendingExternalURL?.absoluteString ?? "")
        }
    }
}

// `internal`, not `private`: the injected bridge source is the contract the
// generated page programs against, and LocalAppsStoreTests derives its
// expectations from it rather than hand-copying them beside it.
struct LocalAppWebViewRepresentable: UIViewRepresentable {
    let appID: String
    let url: URL
    let onBridgeRequest: ((LocalAppBridgeRequest) -> Void)?
    let onExternalNavigation: (URL) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(
            appID: appID,
            allowedOrigin: url,
            onBridgeRequest: onBridgeRequest,
            onExternalNavigation: onExternalNavigation
        )
    }

    func makeUIView(context: Context) -> WKWebView {
        let contentController = WKUserContentController()
        contentController.addUserScript(
            WKUserScript(
                source: Self.bridgeSource,
                injectionTime: .atDocumentStart,
                forMainFrameOnly: true
            )
        )
        for name in Self.messageHandlerNames {
            contentController.add(context.coordinator.broker, name: name)
        }

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = LocalAppWebsiteDataStoreRegistry.shared.dataStore(appID: appID)
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.preferences.isTextInteractionEnabled = true

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        webView.isInspectable = false
        context.coordinator.broker.webView = webView
        context.coordinator.controller.webView = webView
        context.coordinator.controller.markNotReady()
        LocalAppWebViewRegistry.shared.register(context.coordinator.controller, appID: appID)
        context.coordinator.loadedURL = url
        webView.load(URLRequest(url: url, cachePolicy: .reloadRevalidatingCacheData))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.onBridgeRequest = onBridgeRequest
        context.coordinator.onExternalNavigation = onExternalNavigation
        // WebKit normalizes the committed url (a path-less loopback origin gains a
        // trailing "/"), so the last REQUESTED url is what a reload must key on.
        guard context.coordinator.loadedURL != url else { return }
        context.coordinator.loadedURL = url
        context.coordinator.controller.markNotReady()
        webView.load(URLRequest(url: url, cachePolicy: .reloadRevalidatingCacheData))
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        // Unregister first so `bridge_detached` can still be evaluated into
        // the live page before close removes handlers and clears references.
        LocalAppWebViewRegistry.shared.unregister(coordinator.controller, appID: coordinator.broker.appID)
    }

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        let broker: LocalAppBridgeBroker
        let controller: LocalAppWebViewController
        private let allowedOrigin: URL
        /// Last url handed to `webView.load`, mirroring the Android sibling's `view.tag`.
        var loadedURL: URL?
        var onExternalNavigation: (URL) -> Void
        var onBridgeRequest: ((LocalAppBridgeRequest) -> Void)? {
            didSet { broker.onRequest = onBridgeRequest }
        }

        init(
            appID: String,
            allowedOrigin: URL,
            onBridgeRequest: ((LocalAppBridgeRequest) -> Void)?,
            onExternalNavigation: @escaping (URL) -> Void
        ) {
            let broker = LocalAppBridgeBroker(appID: appID, onRequest: onBridgeRequest)
            self.broker = broker
            controller = LocalAppWebViewController(appID: appID, broker: broker)
            self.allowedOrigin = allowedOrigin
            self.onBridgeRequest = onBridgeRequest
            self.onExternalNavigation = onExternalNavigation
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
        ) {
            guard let destination = navigationAction.request.url else {
                decisionHandler(.cancel)
                return
            }
            if isAllowed(destination) {
                decisionHandler(.allow)
            } else {
                decisionHandler(.cancel)
                onExternalNavigation(destination)
            }
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation?) {
            controller.markNotReady()
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation?) {
            controller.markReady()
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation?, withError error: Error) {
            controller.markNotReady()
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation?, withError error: Error) {
            controller.markNotReady()
        }

        func webView(
            _ webView: WKWebView,
            createWebViewWith configuration: WKWebViewConfiguration,
            for navigationAction: WKNavigationAction,
            windowFeatures: WKWindowFeatures
        ) -> WKWebView? {
            if let url = navigationAction.request.url { onExternalNavigation(url) }
            return nil
        }

        private func isAllowed(_ url: URL) -> Bool {
            Self.isAllowed(url, origin: allowedOrigin)
        }

        static func isAllowed(_ url: URL, origin: URL) -> Bool {
            guard url.scheme?.lowercased() != "about" else { return url.absoluteString == "about:blank" }
            guard let scheme = url.scheme?.lowercased(),
                  let originScheme = origin.scheme?.lowercased(),
                  scheme == originScheme,
                  scheme == "http" || scheme == "https",
                  let host = normalizedHost(url),
                  let originHost = normalizedHost(origin),
                  host == originHost,
                  ["127.0.0.1", "localhost", "::1"].contains(originHost)
            else { return false }
            return effectivePort(url, scheme: scheme) == effectivePort(origin, scheme: originScheme)
        }

        private static func normalizedHost(_ url: URL) -> String? {
            url.host?.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        }

        private static func effectivePort(_ url: URL, scheme: String) -> Int? {
            url.port ?? (scheme == "http" ? 80 : scheme == "https" ? 443 : nil)
        }
    }

    static let messageHandlerNames = [
        "lingxiData", "lingxiNetwork", "lingxiRuntime", "lingxiDevice", "lingxiLlm", "lingxiAgent",
    ]

    @MainActor
    static var bridgeSource: String {
        bridgeSource(formFactor: formFactor(for: UIDevice.current.userInterfaceIdiom))
    }

    static func formFactor(for idiom: UIUserInterfaceIdiom) -> String {
        idiom == .pad ? "ipad" : "iphone"
    }

    static func bridgeSource(formFactor: String) -> String {
        precondition(["iphone", "ipad"].contains(formFactor), "Unsupported iOS form factor")
        return bridgeSourceTemplate.replacingOccurrences(
            of: "__LINGXI_NATIVE_FORM_FACTOR__",
            with: formFactor
        )
    }

    private static let bridgeSourceTemplate = #"""
    (() => {
      if (window.lingxi?.v1) return;
      const installCsp = () => {
        if (!document.head || document.head.querySelector('meta[data-lingxi-csp]')) return false;
        const meta = document.createElement('meta');
        meta.httpEquiv = 'Content-Security-Policy';
        meta.dataset.lingxiCsp = 'v1';
        meta.content = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
        document.head.prepend(meta);
        return true;
      };
      if (!installCsp()) {
        const observer = new MutationObserver(() => {
          if (installCsp()) observer.disconnect();
        });
        observer.observe(document.documentElement || document, { childList: true, subtree: true });
      }
      const localOnly = input => {
        const url = new URL(typeof input === 'string' ? input : input.url, location.href);
        if (url.origin !== location.origin) throw new TypeError('External network access must use window.lingxi.v1.network');
        return url;
      };
      const nativeFetch = window.fetch.bind(window);
      window.fetch = (input, init) => { localOnly(input); return nativeFetch(input, init); };
      const nativeOpen = XMLHttpRequest.prototype.open;
      XMLHttpRequest.prototype.open = function(method, url, ...rest) {
        localOnly(url);
        return nativeOpen.call(this, method, url, ...rest);
      };
      const pending = new Map();
      const request = (namespace, operation, payload = {}) => new Promise((resolve, reject) => {
        const requestId = crypto.randomUUID();
        const handler = window.webkit?.messageHandlers?.[`lingxi${namespace}`];
        if (!handler) {
          reject(new Error(`Lingxi ${namespace} bridge unavailable`));
          return;
        }
        let serialized;
        try {
          serialized = JSON.stringify({ requestId, operation, payload });
        } catch (_) {
          const error = new Error('Bridge payload is not serializable');
          error.code = 'payload_invalid';
          reject(error);
          return;
        }
        const byteLimit = namespace === 'Llm' && operation === 'chat' ? 8388608 : 65536;
        if (new TextEncoder().encode(serialized).byteLength > byteLimit) {
          const error = new Error(`Bridge request exceeds ${byteLimit} bytes`);
          error.code = 'request_too_large';
          reject(error);
          return;
        }
        pending.set(requestId, { resolve, reject });
        handler.postMessage({ requestId, operation, payload });
      });
      const readInsets = () => {
        const probe = document.createElement('div');
        probe.style.cssText = 'position:fixed;inset:0;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left);pointer-events:none;';
        (document.documentElement || document.body).appendChild(probe);
        const style = getComputedStyle(probe);
        const number = value => Number.parseFloat(value) || 0;
        const result = { top: number(style.paddingTop), right: number(style.paddingRight), bottom: number(style.paddingBottom), left: number(style.paddingLeft) };
        probe.remove();
        return result;
      };
      const readViewport = () => ({
        width: Math.round(window.visualViewport?.width || window.innerWidth || 0),
        height: Math.round(window.visualViewport?.height || window.innerHeight || 0),
      });
      const deviceContext = Object.freeze({
        os: 'ios',
        formFactor: '__LINGXI_NATIVE_FORM_FACTOR__',
        get viewport() { return readViewport(); },
        get safeArea() { return readInsets(); },
        get colorScheme() { return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'; },
        get reducedMotion() { return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true; },
        get inputMode() { return window.matchMedia?.('(pointer: fine)').matches ? 'pointer' : 'touch'; },
      });
      const api = Object.freeze({
        deviceContext,
        data: Object.freeze({
          query: payload => request('Data', 'query', payload),
          mutate: payload => request('Data', 'mutate', payload),
        }),
        network: Object.freeze({
          fetch: payload => request('Network', 'fetch', payload),
        }),
        runtime: Object.freeze({
          info: () => request('Runtime', 'info', {}),
          deviceContext,
        }),
        device: Object.freeze({
          capturePhoto: (payload = {}) => request('Device', 'capturePhoto', payload),
          pickImage: (payload = {}) => request('Device', 'pickImage', payload),
          recordAudioStart: (payload = {}) => request('Device', 'recordAudioStart', payload),
          recordAudioStop: () => request('Device', 'recordAudioStop', {}),
          getLocation: () => request('Device', 'getLocation', {}),
          transcribeSpeech: (payload = {}) => request('Device', 'transcribeSpeech', payload),
          postNotification: payload => request('Device', 'postNotification', payload),
        }),
        llm: Object.freeze({
          chat: payload => request('Llm', 'chat', payload),
        }),
        agent: Object.freeze({
          post: payload => request('Agent', 'post', payload),
        }),
      });
      const resolveNative = envelope => {
        const entry = pending.get(envelope.requestId);
        if (!entry) return;
        pending.delete(envelope.requestId);
        if (envelope.error) {
          const error = new Error(envelope.error);
          // Stable machine-readable reason (capability_not_declared,
          // audio_session_busy, llm_busy, …) so page code can branch
          // without matching on prose.
          if (envelope.code) error.code = envelope.code;
          entry.reject(error);
        } else {
          entry.resolve(envelope.result);
        }
      };
      Object.defineProperty(window, 'lingxi', {
        value: Object.freeze({ v1: api, __resolve: resolveNative }),
        configurable: false,
        writable: false,
      });
    })();
    """#
}

/// Owns the durable app-id -> WebKit data-store mapping and deletion journal.
///
/// The journal deliberately lives outside the Rust app directory: Rust removes
/// that directory first, while WebKit is the only component capable of deleting
/// its own persistent store. A cleanup entry is removed only after WebKit reports
/// success, so a process kill or WebKit failure is retried after the next app
/// snapshot arrives.
@MainActor
final class LocalAppWebsiteDataStoreRegistry {
    struct PendingCleanup: Codable, Hashable, Sendable {
        let appID: String
        let dataStoreIdentifier: UUID
        var absenceConfirmed: Bool

        init(appID: String, dataStoreIdentifier: UUID, absenceConfirmed: Bool = false) {
            self.appID = appID
            self.dataStoreIdentifier = dataStoreIdentifier
            self.absenceConfirmed = absenceConfirmed
        }

        private enum CodingKeys: String, CodingKey {
            case appID
            case dataStoreIdentifier
            case absenceConfirmed
        }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            appID = try values.decode(String.self, forKey: .appID)
            dataStoreIdentifier = try values.decode(UUID.self, forKey: .dataStoreIdentifier)
            absenceConfirmed = try values.decodeIfPresent(Bool.self, forKey: .absenceConfirmed) ?? false
        }
    }

    typealias DataStoreRemover = @MainActor (UUID) async -> Bool

    static let shared = LocalAppWebsiteDataStoreRegistry()

    private static let mappingPrefix = "local-apps.web-data-store."
    private static let pendingCleanupKey = "local-apps.pending-web-data-cleanup.v1"

    private let defaults: UserDefaults
    private let removeDataStore: DataStoreRemover

    init(
        defaults: UserDefaults = .standard,
        removeDataStore: DataStoreRemover? = nil
    ) {
        self.defaults = defaults
        self.removeDataStore = removeDataStore ?? Self.removePersistentDataStore
    }

    func dataStore(appID: String) -> WKWebsiteDataStore {
        let currentIdentifier = identifier(appID: appID)
        let isConfirmedForRemoval = pendingCleanups.contains {
            $0.appID == appID
                && $0.dataStoreIdentifier == currentIdentifier
                && $0.absenceConfirmed
        }
        guard isConfirmedForRemoval else {
            return WKWebsiteDataStore(forIdentifier: currentIdentifier)
        }

        // The app disappeared from an authoritative snapshot, so its old
        // store is committed to deletion even if WebKit is still completing
        // that async operation. A same-id recreation must get a fresh store
        // immediately and must never reattach to the pending old identifier.
        let replacement = UUID()
        defaults.set(replacement.uuidString, forKey: Self.mappingKey(appID: appID))
        return WKWebsiteDataStore(forIdentifier: replacement)
    }

    /// Journals the exact store identifier before the engine is asked to delete
    /// the app. Materializing the store makes removal well-defined even for an
    /// app that has never opened its preview.
    func prepareForDeletion(appID: String) {
        let dataStore = dataStore(appID: appID)
        guard let dataStoreIdentifier = dataStore.identifier else { return }
        let entry = PendingCleanup(appID: appID, dataStoreIdentifier: dataStoreIdentifier)
        var pending = pendingCleanups
        guard !pending.contains(entry) else { return }
        pending.append(entry)
        savePendingCleanups(pending)
    }

    /// Rolls back the journal when the engine did not accept the delete.
    /// Once an authoritative snapshot confirmed absence, cleanup belongs to
    /// that completed deletion and must survive any later same-id operation.
    func cancelDeletion(appID: String) {
        savePendingCleanups(pendingCleanups.filter {
            $0.appID != appID || $0.absenceConfirmed
        })
    }

    /// Removes only entries whose app absence has been confirmed by the latest
    /// authoritative `AppsChanged` snapshot.
    func removeDataForDeletedApps(activeAppIDs: Set<String>) async {
        let eligible = pendingCleanups.filter {
            $0.absenceConfirmed || !activeAppIDs.contains($0.appID)
        }
        for var entry in eligible {
            guard !Task.isCancelled else { return }
            entry.absenceConfirmed = true
            replacePendingCleanup(entry)
            LocalAppWebViewRegistry.shared.close(appID: entry.appID)
            guard await removeDataStore(entry.dataStoreIdentifier) else { continue }

            let mappingKey = Self.mappingKey(appID: entry.appID)
            if defaults.string(forKey: mappingKey) == entry.dataStoreIdentifier.uuidString {
                defaults.removeObject(forKey: mappingKey)
            }
            savePendingCleanups(pendingCleanups.filter { $0 != entry })
        }
    }

    var pendingCleanups: [PendingCleanup] {
        guard let data = defaults.data(forKey: Self.pendingCleanupKey),
              let values = try? PropertyListDecoder().decode([PendingCleanup].self, from: data)
        else { return [] }
        return values
    }

    func storedIdentifier(appID: String) -> UUID? {
        defaults.string(forKey: Self.mappingKey(appID: appID)).flatMap(UUID.init(uuidString:))
    }

    private func identifier(appID: String) -> UUID {
        let key = Self.mappingKey(appID: appID)
        if let rawValue = defaults.string(forKey: key), let stored = UUID(uuidString: rawValue) {
            return stored
        }
        let created = UUID()
        defaults.set(created.uuidString, forKey: key)
        return created
    }

    private func savePendingCleanups(_ values: [PendingCleanup]) {
        if values.isEmpty {
            defaults.removeObject(forKey: Self.pendingCleanupKey)
            return
        }
        guard let data = try? PropertyListEncoder().encode(values) else { return }
        defaults.set(data, forKey: Self.pendingCleanupKey)
    }

    private func replacePendingCleanup(_ replacement: PendingCleanup) {
        var pending = pendingCleanups
        guard let index = pending.firstIndex(where: {
            $0.appID == replacement.appID
                && $0.dataStoreIdentifier == replacement.dataStoreIdentifier
        }) else { return }
        pending[index] = replacement
        savePendingCleanups(pending)
    }

    private static func mappingKey(appID: String) -> String {
        "\(mappingPrefix)\(appID)"
    }

    private static func removePersistentDataStore(identifier: UUID) async -> Bool {
        await withCheckedContinuation { continuation in
            WKWebsiteDataStore.remove(forIdentifier: identifier) { error in
                continuation.resume(returning: error == nil)
            }
        }
    }
}
