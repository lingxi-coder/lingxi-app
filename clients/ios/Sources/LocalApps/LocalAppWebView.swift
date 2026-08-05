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
        controllers[appID] = WeakController(controller)
    }

    func unregister(_ controller: LocalAppWebViewController, appID: String) {
        guard controllers[appID]?.value === controller else { return }
        controllers[appID] = nil
    }

    func resolveBridge(
        appID: String,
        requestID: String,
        resultJSON: String?,
        error: String?
    ) {
        guard let controller = controllers[appID]?.value else { return }
        let result = resultJSON.flatMap(Self.decodeJSON)
        let invalidResult = resultJSON != nil && result == nil
        controller.broker.resolve(
            requestID: requestID,
            result: result,
            error: invalidResult ? String(localized: "local_apps_error_bridge_invalid_json") : error
        )
    }

    #if canImport(engine_mobileFFI)
        func execute(request: AppUiRequestDto) async -> LocalAppUIExecutionResult {
            for _ in 0 ..< 50 {
                if let controller = controllers[request.appId]?.value {
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
    let appID: String
    var onRequest: ((LocalAppBridgeRequest) -> Void)?

    weak var webView: WKWebView?

    init(appID: String, onRequest: ((LocalAppBridgeRequest) -> Void)? = nil) {
        self.appID = appID
        self.onRequest = onRequest
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame,
              let body = message.body as? [String: Any],
              let requestID = body["requestId"] as? String,
              let operation = body["operation"] as? String,
              requestID.count <= 128,
              operation.count <= 128
        else { return }

        let namespace = message.name.replacingOccurrences(of: "lingxi", with: "").lowercased()
        let rawPayload = body["payload"] as? [String: Any] ?? [:]
        let payloadJSON: String?
        if JSONSerialization.isValidJSONObject(rawPayload),
           let data = try? JSONSerialization.data(withJSONObject: rawPayload),
           data.count <= 64 * 1_024 {
            payloadJSON = String(data: data, encoding: .utf8)
        } else {
            payloadJSON = nil
        }
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

    func resolve(requestID: String, result: Any?, error: String?) {
        guard let webView else { return }
        let envelope: [String: Any] = [
            "requestId": requestID,
            "result": result ?? NSNull(),
            "error": error ?? NSNull(),
        ]
        guard JSONSerialization.isValidJSONObject(envelope),
              let data = try? JSONSerialization.data(withJSONObject: envelope),
              let json = String(data: data, encoding: .utf8)
        else { return }
        webView.evaluateJavaScript("window.lingxi?.__resolve(\(json));")
    }
}

@MainActor
final class LocalAppWebViewController {
    let appID: String
    let broker: LocalAppBridgeBroker
    weak var webView: WKWebView?

    init(appID: String, broker: LocalAppBridgeBroker) {
        self.appID = appID
        self.broker = broker
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
                webView.goBack()
                return encodedResult(["ok": true, "action": "back"])
            }
            if request.action == .reload {
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

private struct LocalAppWebViewRepresentable: UIViewRepresentable {
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
        configuration.websiteDataStore = LocalAppWebsiteDataStoreRegistry.dataStore(appID: appID)
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.preferences.isTextInteractionEnabled = true

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        webView.isInspectable = false
        context.coordinator.broker.webView = webView
        context.coordinator.controller.webView = webView
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
        webView.load(URLRequest(url: url, cachePolicy: .reloadRevalidatingCacheData))
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.stopLoading()
        for name in messageHandlerNames {
            webView.configuration.userContentController.removeScriptMessageHandler(forName: name)
        }
        coordinator.broker.webView = nil
        coordinator.controller.webView = nil
        LocalAppWebViewRegistry.shared.unregister(coordinator.controller, appID: coordinator.broker.appID)
        webView.navigationDelegate = nil
        webView.uiDelegate = nil
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
            guard url.scheme == "http" || url.scheme == "https" else {
                return url.scheme == "about"
            }
            let loopbackHosts = ["127.0.0.1", "localhost", "::1"]
            guard let host = url.host?.lowercased(), loopbackHosts.contains(host) else { return false }
            return url.port == allowedOrigin.port
        }
    }

    private static let messageHandlerNames = ["lingxiData", "lingxiNetwork", "lingxiRuntime"]

    private static let bridgeSource = #"""
    (() => {
      if (window.lingxi?.v1) return;
      const installCsp = () => {
        if (!document.head || document.head.querySelector('meta[data-lingxi-csp]')) return false;
        const meta = document.createElement('meta');
        meta.httpEquiv = 'Content-Security-Policy';
        meta.dataset.lingxiCsp = 'v1';
        meta.content = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
        document.head.prepend(meta);
        return true;
      };
      if (!installCsp()) {
        const observer = new MutationObserver(() => {
          if (installCsp()) observer.disconnect();
        });
        observer.observe(document.documentElement, { childList: true, subtree: true });
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
        pending.set(requestId, { resolve, reject });
        const handler = window.webkit?.messageHandlers?.[`lingxi${namespace}`];
        if (!handler) {
          pending.delete(requestId);
          reject(new Error(`Lingxi ${namespace} bridge unavailable`));
          return;
        }
        handler.postMessage({ requestId, operation, payload });
      });
      const api = Object.freeze({
        data: Object.freeze({
          query: payload => request('Data', 'query', payload),
          mutate: payload => request('Data', 'mutate', payload),
        }),
        network: Object.freeze({
          fetch: payload => request('Network', 'fetch', payload),
        }),
        runtime: Object.freeze({
          info: () => request('Runtime', 'info', {}),
        }),
      });
      const resolveNative = envelope => {
        const entry = pending.get(envelope.requestId);
        if (!entry) return;
        pending.delete(envelope.requestId);
        envelope.error ? entry.reject(new Error(envelope.error)) : entry.resolve(envelope.result);
      };
      Object.defineProperty(window, 'lingxi', {
        value: Object.freeze({ v1: api, __resolve: resolveNative }),
        configurable: false,
        writable: false,
      });
    })();
    """#
}

private enum LocalAppWebsiteDataStoreRegistry {
    static func dataStore(appID: String) -> WKWebsiteDataStore {
        let key = "local-apps.web-data-store.\(appID)"
        let defaults = UserDefaults.standard
        let identifier: UUID
        if let rawValue = defaults.string(forKey: key), let stored = UUID(uuidString: rawValue) {
            identifier = stored
        } else {
            identifier = UUID()
            defaults.set(identifier.uuidString, forKey: key)
        }
        return WKWebsiteDataStore(forIdentifier: identifier)
    }
}
