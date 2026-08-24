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

/// `takeSnapshot` can call back with neither an image nor an error (an offscreen
/// or not-yet-composited view). Continuations must be resumed exactly once, so
/// that case needs something concrete to throw rather than a silent hang.
enum LocalAppSnapshotError: Error, LocalizedError {
    case unavailable

    var errorDescription: String? {
        String(localized: "local_apps_error_ui_capture_unavailable")
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

    func deliverStreamFrame(appID: String, frameJSON: String) {
        guard let controller = controllers[appID]?.value,
              let webView = controller.webView,
              let data = frameJSON.data(using: .utf8),
              let json = String(data: data, encoding: .utf8)
        else { return }
        webView.evaluateJavaScript("window.lingxi?.__stream(\(json));")
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
        if namespace == "llm" && (operation == "chat" || operation == "stream") { return maxLLMBytes }
        if namespace == "files" && (operation == "read" || operation == "write") { return 4 * 1024 * 1024 }
        return maxControlBytes
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
            if request.action == .captureView {
                return await captureFrame(in: webView, value: request.value)
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
            case .captureView: "capture_view"
            case .pointer: "pointer"
            case .key: "key"
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

        /// Long-edge cap for a snapshot region, in POINTS.
        ///
        /// The whole-view path derives `snapshotWidth` from the view's bounds; a
        /// crop must derive it from the crop, or WebKit scales the region UP to
        /// the view-sized width. `min(1, …)` is what forbids the enlargement.
        ///
        /// Internal rather than private so `LocalAppsStoreTests` can pin the
        /// arithmetic directly, matching `executionSource(requestJSON:)` above.
        static func snapshotWidthPoints(rect: CGRect, capPoints: CGFloat) -> CGFloat {
            let longEdge = max(rect.width, rect.height)
            guard longEdge > 0 else { return rect.width }
            let scale = min(1, capPoints / longEdge)
            return rect.width * scale
        }

        /// Parses the optional `{"rect":{"x","y","width","height"}}` payload
        /// `capture_ui_value` (Rust, `local_apps_host.rs`) puts in
        /// `AppUiRequestDto.value` for a region capture. The four numbers are
        /// CSS pixels, which are the same unit `webView.bounds` already is.
        ///
        /// `capture_ui_value` deliberately preserves the caller's original
        /// numeric form, so a field can arrive as either a JSON integer or a
        /// JSON float. `JSONSerialization` bridges BOTH shapes to the same
        /// `NSNumber` representation regardless of which one the source text
        /// used (confirmed empirically -- a hand-parsed `{"x":10}` and
        /// `{"x":10.5}` both come back as `__NSCFNumber`, and `as? Double`
        /// succeeds for both; see task-8-report.md, fix round 1, which also
        /// corrects an earlier, unverified claim here that `as? Double` was
        /// shape-sensitive -- it is not). Going through `NSNumber` and
        /// `.doubleValue` explicitly, rather than `rect[name] as? Double`
        /// directly, is still deliberate: `as? Int` (not `Double`) is the
        /// cast that IS shape-sensitive -- it fails outright on a fractional
        /// value like `10.5` -- so reading as a floating type is what keeps
        /// a fractional CSS pixel from being rejected.
        ///
        /// Returns `nil` for an absent, malformed, non-finite, or shapeless
        /// rect — exactly like no `value` at all, i.e. "capture the whole
        /// view". The Rust side already rejects a non-finite or non-positive
        /// rect before it is ever sent, so this is a defensive fallback, not
        /// the primary validation.
        static func parseRequestedRect(fromValueJSON json: String?) -> CGRect? {
            guard let json,
                  let data = json.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data, options: .fragmentsAllowed) as? [String: Any],
                  let rect = object["rect"] as? [String: Any]
            else { return nil }
            func field(_ name: String) -> CGFloat? {
                guard let number = rect[name] as? NSNumber else { return nil }
                let value = number.doubleValue
                return value.isFinite ? CGFloat(value) : nil
            }
            guard let x = field("x"), let y = field("y"), let width = field("width"), let height = field("height")
            else { return nil }
            return CGRect(x: x, y: y, width: width, height: height)
        }

        /// Capture the app view as a JPEG small enough to survive the result
        /// channel.
        ///
        /// `takeSnapshot` and not `canvas.toDataURL`: the snapshot comes off the
        /// native compositor, so a WebGL app is captured without needing
        /// `preserveDrawingBuffer` (which a generated app would have to opt into,
        /// and which costs a frame copy for every frame it draws). It also
        /// captures the page as composited rather than one canvas element.
        ///
        /// The result rides the same `result_json` String channel as every other
        /// UI action, which this file caps at 256 KiB. Base64 inflates by 4/3, so
        /// the JPEG itself has to land well under that — hence the downscale and
        /// the quality ladder rather than a single fixed quality.
        private func captureFrame(in webView: WKWebView, value: String?) async -> LocalAppUIExecutionResult {
            let bounds = webView.bounds
            guard bounds.width > 0, bounds.height > 0 else {
                // The page IS open — it just has no laid-out geometry to draw,
                // which is what the offscreen copy says and what Android
                // already returns for the same condition. `ui_not_open` sends
                // the agent off to restart a runtime that is running fine.
                return .failure(String(localized: "local_apps_error_ui_capture_unavailable"))
            }

            // An optional crop rides `value` as `{"rect":{"x","y","width","height"}}`
            // (`capture_ui_value` in local_apps_host.rs), in CSS pixels — the same
            // units `webView.bounds` already is. That Rust-side function checks
            // only shape and finiteness; CLAMPING to the real viewport is this
            // client's job, since only the client knows it. `intersection` clamps
            // a partially-out-of-bounds rect down to what is actually on screen; a
            // rect with no overlap at all collapses to `CGRect.null`, whose width
            // and height are both 0 (`CGRect(x:.infinity,y:.infinity,width:0,
            // height:0)` — confirmed by running `CGRect(...).intersection(...)`
            // directly rather than assuming), so the guard below fails the same
            // way an unlaid-out webview already does, instead of silently
            // substituting the whole frame for a region the agent never asked for.
            let requestedRect = Self.parseRequestedRect(fromValueJSON: value)
            let region = requestedRect.map { $0.intersection(bounds) } ?? bounds
            guard region.width > 0, region.height > 0 else {
                return .failure(String(localized: "local_apps_error_ui_capture_unavailable"))
            }

            let configuration = WKSnapshotConfiguration()
            configuration.rect = region
            // Cap the long edge in PIXELS, which is what the encoder below
            // actually sees. `snapshotWidth` is in POINTS and `takeSnapshot`
            // hands back a UIImage at the screen scale, so asking for 1024
            // points yields 3072 pixels on a 3x device — 9x the area the
            // quality ladder is sized for, and 9x what Android produces from
            // the same nominal cap (`View.getWidth()` is already pixels there).
            // Dividing by the scale first is what makes the two platforms
            // return comparable evidence.
            //
            // The cap is computed from the REGION's own long edge, not the
            // view's. Reusing the whole-view ladder for a crop would ask
            // WebKit to scale a small region UP to the view-sized width — a
            // blurry enlargement that also spends the JPEG budget on invented
            // pixels. `snapshotWidthPoints` is what forbids that (its
            // `min(1, …)`).
            let displayScale = max(webView.traitCollection.displayScale, 1)
            let capPoints: CGFloat = 1_024 / displayScale
            configuration.snapshotWidth = NSNumber(
                value: Double(Self.snapshotWidthPoints(rect: region, capPoints: capPoints))
            )

            let image: UIImage
            do {
                image = try await withCheckedThrowingContinuation { continuation in
                    webView.takeSnapshot(with: configuration) { snapshot, error in
                        if let snapshot {
                            continuation.resume(returning: snapshot)
                        } else {
                            continuation.resume(
                                throwing: error ?? LocalAppSnapshotError.unavailable
                            )
                        }
                    }
                }
            } catch {
                return .failure(error.localizedDescription)
            }

            // Step down until the base64 fits with room for the JSON envelope.
            // Reported rather than silently truncated: a frame that had to drop
            // to 0.3 is a signal about the app, not just about the transport.
            let budget = 170 * 1_024
            var encoded: Data?
            var usedQuality: CGFloat = 0
            for quality in [CGFloat(0.7), 0.5, 0.3] {
                guard let data = image.jpegData(compressionQuality: quality) else { continue }
                usedQuality = quality
                encoded = data
                if data.count <= budget { break }
            }
            guard let data = encoded, data.count <= budget else {
                return .failure(String(localized: "local_apps_error_ui_capture_too_large"))
            }

            var payload: [String: Any] = [
                "ok": true,
                "action": "capture_view",
                "image": [
                    "data": data.base64EncodedString(),
                    "mime_type": "image/jpeg",
                    // The frame's OWN pixel size. Without it the agent cannot
                    // turn a feature it sees in the image into a `pointer`
                    // coordinate, because `pointer` is in CSS pixels and the
                    // frame was downscaled by an amount nothing reported —
                    // so it guesses, the tap lands elsewhere, and the call
                    // still answers ok:true.
                    "width": Int(image.size.width * image.scale),
                    "height": Int(image.size.height * image.scale),
                ],
                // Without these the frame is unreadable as evidence: the same app
                // is a different layout on an iPad in landscape and an iPhone in
                // portrait, and the pixels alone do not say which one this is.
                // `viewport` is ALWAYS the whole view, regardless of `capture_rect`
                // below -- it answers "what device/layout is this", not "what does
                // `image` show".
                //
                // In CSS pixels, which is the unit `pointer` takes, for a
                // WHOLE-VIEW capture (no `capture_rect` in this result): divide an
                // image coordinate by `image.width / viewport.width`. That ratio
                // is *only* valid when `image` and `viewport` describe the same
                // origin — true for the whole view, false for a crop, and the
                // existing whole-view downscale means a smaller `image` than
                // `viewport * device_pixel_ratio` is not itself proof a crop
                // happened, hence `capture_rect`'s presence being the actual
                // signal (see below).
                "viewport": [
                    "width": Int(bounds.width.rounded()),
                    "height": Int(bounds.height.rounded()),
                ],
                "device_pixel_ratio": Double(webView.traitCollection.displayScale),
                "jpeg_quality": Double(usedQuality),
            ]
            // Present ONLY when a crop was requested (never for a whole-view
            // capture, which is what keeps that path's JSON byte-for-byte
            // unchanged) and always the CLAMPED region actually handed to
            // `takeSnapshot` -- not the caller's original request -- because a
            // partly-out-of-bounds request is silently narrowed by `intersection`
            // above, and the agent needs the rect that was ACTUALLY captured to
            // convert a coordinate back correctly.
            //
            // For a crop, the whole-view ratio above does not apply: it uses the
            // VIEW's width and has no origin term, so it is wrong both by scale
            // (view width vs. the crop's own width) and by a missing additive
            // offset (the crop is not anchored at the view's origin). The correct
            // conversion, in CSS pixels, replaces `viewport` with `capture_rect`
            // as the ratio's base AND adds its origin back in:
            //   CSS_x = capture_rect.x + imageX * capture_rect.width / image.width
            //   CSS_y = capture_rect.y + imageY * capture_rect.height / image.height
            // (`skills/frontend-qa/SKILL.md` carries this same formula for the
            // agent, since a formula that lives only in this comment is a formula
            // the model calling this tool never sees.)
            if requestedRect != nil {
                payload["capture_rect"] = [
                    "x": Double(region.origin.x),
                    "y": Double(region.origin.y),
                    "width": Double(region.width),
                    "height": Double(region.height),
                ]
            }
            return encodedResult(payload)
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

    /// Internal rather than private so `LocalAppsStoreTests` can pin the
    /// shadow-DOM walk, matching the Android side where
    /// `buildLocalAppUiExecutionScript` is already reachable from its test.
    static func executionSource(requestJSON: String) -> String {
        #"""
        (() => {
          const request = \#(requestJSON);
          const clean = value => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, 500);
          const roleOf = element => clean(element.getAttribute('role') || ({
            BUTTON: 'button', A: 'link', INPUT: element.type === 'checkbox' ? 'checkbox' : (element.type === 'radio' ? 'radio' : 'textbox'),
            SELECT: 'combobox', TEXTAREA: 'textbox'
          })[element.tagName] || '');
          // A password / hidden input's CONTENT must never leave the WebView, in any
          // field. `snapshot` already redacts `value`, but the accessible name falls
          // back to `element.value` for an input with no label, no placeholder and
          // no text — so an unlabelled `<input type="password">` used to ship the
          // typed password to the model as `name`. `deepQuery` widened the reach of
          // this walk into shadow roots, so the fallback now sees component-internal
          // inputs too.
          const isSensitive = element => element instanceof HTMLInputElement && ['hidden', 'password'].includes(element.type);
          const nameOf = element => {
            // `deepQuery` below returns elements from INSIDE shadow roots, and a
            // shadow root is its own id scope. Resolving their labels against
            // `document` searches the wrong tree, so an element the walk just
            // surfaced comes back unnamed and role+name targeting misses it.
            const scope = element.getRootNode?.() || document;
            const byId = id => (scope.getElementById ? scope.getElementById(id) : document.getElementById(id));
            const labelledBy = element.getAttribute('aria-labelledby');
            const labelled = labelledBy ? byId(labelledBy) : null;
            const explicit = element.id ? scope.querySelector(`label[for="${CSS.escape(element.id)}"]`) : null;
            return clean(element.getAttribute('aria-label') || labelled?.textContent || explicit?.textContent || element.placeholder || element.innerText || (isSensitive(element) ? '' : element.value));
          };
          // Ionic renders a component's interactive internals inside a SHADOW ROOT, and
          // `document.querySelectorAll` does not cross that boundary. Without this walk
          // an app built from ion-* components reports `elements: []` — the same signal
          // a blank screen and a crashed app produce, which is exactly the ambiguity
          // `canvasCount` exists to resolve for a drawn surface.
          const SELECTOR = 'button,a[href],input,select,textarea,[role],[tabindex],[contenteditable="true"]';
          const deepQuery = (selector, limit) => {
            const found = [];
            const seen = new Set();
            const visit = (root, depth) => {
              // Bounded on BOTH axes: a component library nests a few roots deep, but a
              // page that nests further (or loops) must degrade to a partial list rather
              // than hang the WebView while the tool call waits on it.
              if (!root || depth > 8 || found.length >= limit) return;
              let matched = [];
              try { matched = Array.from(root.querySelectorAll(selector)); } catch (error) { return; }
              for (const element of matched) {
                if (found.length >= limit) return;
                if (!seen.has(element)) { seen.add(element); found.push(element); }
              }
              let hosts = [];
              try { hosts = Array.from(root.querySelectorAll('*')); } catch (error) { return; }
              for (const host of hosts) if (host.shadowRoot) visit(host.shadowRoot, depth + 1);
            };
            visit(document, 0);
            return found;
          };
          const candidates = () => deepQuery(SELECTOR, 400);
          // `ion-input` and its siblings keep the real <input> in their shadow root, so
          // a fill aimed at the host would write to a custom element that has no value
          // setter and no `input` event to dispatch.
          const nativeControl = element => {
            if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement) return element;
            return element.shadowRoot ? (element.shadowRoot.querySelector('input,textarea,select') || element) : element;
          };
          const findTarget = target => {
            if (!target) return null;
            if (target.elementId) {
              // `getElementById` is document-scoped; a shadow root is a separate id
              // scope, and Ionic mints ids like `ion-input-0` inside one.
              const byId = document.getElementById(target.elementId)
                || deepQuery('[id="' + String(target.elementId).replace(/["\\]/g, '\\$&') + '"]', 1)[0];
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
          const vvOf = () => {
            const vv = window.visualViewport;
            return vv
              ? { width: Math.round(vv.width), height: Math.round(vv.height),
                  offsetLeft: Math.round(vv.offsetLeft), offsetTop: Math.round(vv.offsetTop),
                  scale: vv.scale }
              : { width: Math.round(window.innerWidth), height: Math.round(window.innerHeight),
                  offsetLeft: 0, offsetTop: 0, scale: 1 };
          };
          const rectOf = element => {
            const rect = element.getBoundingClientRect();
            return [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)];
          };
          const snapshot = () => {
            const out = {
              title: clean(document.title),
              url: location.href,
              documentState: document.readyState,
              viewport: vvOf(),
              runtimeErrors: (window.__lingxiRuntimeErrors || []).slice(0, 8),
              runtimeErrorsDropped: window.__lingxiRuntimeErrorsDropped || 0,
              // The host gate compares ONLY the pixels inside these rects, so a
              // DOM spinner cannot stand in for a frozen canvas. `canvasCount`
              // stays for compatibility with readers that only counted.
              canvases: deepQuery('canvas', 16).map(c => ({ rect: rectOf(c) })),
              canvasCount: deepQuery('canvas', 64).length,
              elements: candidates().slice(0, 200).map(element => {
                const rect = element.getBoundingClientRect();
                const sensitive = isSensitive(element);
                return {
                  elementId: clean(element.id) || null,
                  role: roleOf(element) || null,
                  name: nameOf(element) || null,
                  value: sensitive ? null : clean(element.value),
                  checked: typeof element.checked === 'boolean' ? element.checked : null,
                  disabled: !!element.disabled,
                  visible: rect.width > 0 && rect.height > 0,
                  rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)],
                };
              }),
              truncated: [],
            };
            // 256 KiB is a HARD failure on the result channel on iOS, and
            // Android enforces no cap on `inspect` at all — so this ladder is
            // the only thing bounding the payload there either way. Leave
            // room for the JSON envelope and degrade in a fixed order rather
            // than dying (iOS) or growing unbounded (Android). Measured in
            // UTF-8 BYTES via TextEncoder, matching the native guard this
            // budget exists to stay under (`resultJSON.utf8.count` on iOS) —
            // `.length` counts UTF-16 code units, and CJK text is 1 unit but
            // 3 bytes per character, so a length-only check could call a
            // payload "safe" at roughly a third of its real size.
            const BUDGET = 200 * 1024;
            const truncated = out.truncated;
            const size = () => new TextEncoder().encode(JSON.stringify(out)).length;
            for (const seg of ['elements', 'canvases', 'runtimeErrors']) {
              if (size() <= BUDGET) break;
              if (seg === 'elements') out.elements = out.elements.slice(0, 50);
              else if (seg === 'canvases') out.canvases = [];
              else out.runtimeErrors = [];
              truncated.push(seg);
            }
            return out;
          };
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
            // `pointer` and `key` resolve NO element, so they must return before
            // the findTarget block below — a canvas app has nothing for it to
            // find, which is the entire reason these two actions exist.
            if (request.action === 'pointer') {
              const parts = clean(request.value).split(',').map(part => part.trim());
              // `Number('')` is 0, not NaN, so a missing field would pass the
              // finiteness check below and silently place the tap on the top
              // edge: "10," would become (10, 0) — the very coordinate `click`
              // gets wrong and `pointer` exists to avoid.
              const coord = part => (part === undefined || part === '' ? NaN : Number(part));
              const x = coord(parts[0]);
              const y = coord(parts[1]);
              const phase = (parts[2] || 'tap').toLowerCase();
              if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error('pointer needs value "x,y" in CSS pixels');
              if (!['tap', 'down', 'move', 'up'].includes(phase)) throw new Error('pointer phase must be tap, down, move or up');
              // Dispatch on the element under the point so the event BUBBLES the
              // way a real one would; a canvas listener on window still sees it.
              // A null hit means the point is OUTSIDE the viewport — say so
              // instead of quietly retargeting to body, which dispatched a tap
              // that reached nothing and still answered ok:true, so the agent
              // recorded an interaction that never happened.
              const receiver = document.elementFromPoint(x, y);
              if (!receiver) throw new Error('pointer ' + x + ',' + y + ' is outside the ' + Math.round(window.innerWidth) + 'x' + Math.round(window.innerHeight) + ' CSS-pixel viewport');
              const base = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, pointerId: 1, pointerType: 'touch', isPrimary: true, button: 0, buttons: 1 };
              const fire = (type, overrides) => {
                const init = Object.assign({}, base, overrides || {});
                receiver.dispatchEvent(new PointerEvent(type, init));
                // Many canvas engines bind mouse events only; a PointerEvent
                // alone would silently do nothing for them.
                const mouseType = type === 'pointerdown' ? 'mousedown' : type === 'pointerup' ? 'mouseup' : 'mousemove';
                receiver.dispatchEvent(new MouseEvent(mouseType, init));
              };
              // `buttons` on a move must say whether a button is HELD. A drag is
              // driven as down -> move -> up, and canvas/slider handlers almost
              // universally start with `if (!e.buttons) return;` — a move that
              // always reports 0 is a hover, so every drag registered as a click
              // at the start point with no travel. The held state is remembered
              // across calls because each action arrives as its own script.
              if (phase === 'down') window.__lingxiPointerHeld = true;
              const held = window.__lingxiPointerHeld ? 1 : 0;
              if (phase === 'move') fire('pointermove', { buttons: held });
              else if (phase === 'down') fire('pointerdown');
              else if (phase === 'up') { fire('pointerup', { buttons: 0 }); window.__lingxiPointerHeld = false; }
              else {
                fire('pointerdown');
                fire('pointerup', { buttons: 0 });
                receiver.dispatchEvent(new MouseEvent('click', Object.assign({}, base, { buttons: 0 })));
                window.__lingxiPointerHeld = false;
              }
              return JSON.stringify({ ok: true, action: 'pointer', phase, x, y, receiver: receiver.tagName || null });
            }
            if (request.action === 'key') {
              const raw = clean(request.value);
              const comma = raw.lastIndexOf(',');
              const maybePhase = comma >= 0 ? raw.slice(comma + 1).trim().toLowerCase() : '';
              const hasPhase = ['press', 'down', 'up'].includes(maybePhase);
              // Split from the RIGHT and only when the tail is a known phase, so
              // the key `,` itself still works.
              const named = hasPhase ? raw.slice(0, comma).trim() : raw;
              const phase = hasPhase ? maybePhase : 'press';
              if (!named) throw new Error('key needs a DOM key name, e.g. ArrowLeft');
              // SPACE: the DOM key name is a single space, which cannot survive
              // the wire — `clean()` trims, so " " arrives as "" and used to be
              // rejected outright. The code name `Space` is the only spelling
              // that gets here, so translate it back to the real key. Without
              // this the most common game key (jump/fire/pause) was unreachable:
              // " " threw, and "Space" produced `e.key === 'Space'`, which
              // matches nothing.
              const key = named === 'Space' ? ' ' : named;
              // Falling back to `document.body` reached NOTHING for the case
              // this action exists for: a synthetic pointer does not move focus,
              // so after tapping a canvas `activeElement` is still body — and an
              // event dispatched ON body propagates UP, never down into the
              // canvas, so a canvas-scoped keydown listener never fired while
              // the call still answered ok:true. Dispatching on the canvas
              // instead reaches listeners at every level, because the event
              // bubbles canvas -> body -> document -> window.
              const receiver = document.activeElement && document.activeElement !== document.body
                ? document.activeElement
                : (deepQuery('canvas', 1)[0] || document.body);
              // Ordered so the space case is reached: a space HAS length 1, so
              // testing it after the length check made that arm dead code and
              // emitted `code: ''`, which `e.code === 'Space'` never matches.
              const code = key === ' '
                ? 'Space'
                : key.length === 1
                ? (/[a-z]/i.test(key) ? 'Key' + key.toUpperCase() : /[0-9]/.test(key) ? 'Digit' + key : '')
                : key;
              const init = { key, code, bubbles: true, cancelable: true, composed: true };
              if (phase === 'down' || phase === 'press') receiver.dispatchEvent(new KeyboardEvent('keydown', init));
              if (phase === 'up' || phase === 'press') receiver.dispatchEvent(new KeyboardEvent('keyup', init));
              return JSON.stringify({ ok: true, action: 'key', key, phase });
            }
            const element = findTarget(request.target);
            if (!element) throw new Error('UI target was not found');
            if (element.disabled) throw new Error('UI target is disabled');
            if (request.action === 'click') {
              element.click();
            } else if (request.action === 'fill') {
              const field = nativeControl(element);
              if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement || field.isContentEditable)) throw new Error('Target cannot be filled');
              if (field.isContentEditable) field.textContent = request.value || '';
              else setNativeValue(field, request.value || '');
              dispatchValueChange(field);
            } else if (request.action === 'select') {
              const field = nativeControl(element);
              if (!(field instanceof HTMLSelectElement)) throw new Error('Target is not a select element');
              const option = Array.from(field.options).find(item => item.value === request.value || clean(item.textContent) === clean(request.value));
              if (!option) throw new Error('Select option was not found');
              setNativeValue(field, option.value);
              dispatchValueChange(field);
            } else if (request.action === 'toggle') {
              // A supplied value is the REQUESTED state, so an element already in it must not flip.
              const desired = clean(request.value).toLowerCase();
              // `ion-checkbox`/`ion-toggle` keep the real <input> — and the
              // `role`/`aria-checked` that describe it — inside their SHADOW ROOT,
              // exactly like `ion-input`. Reading the host alone answered "not
              // toggleable" for every checkbox and switch in every Ionic app.
              // The CLICK still goes to the host, which is what the component listens on.
              const stateOf = node => node instanceof HTMLInputElement && (node.type === 'checkbox' || node.type === 'radio')
                ? !!node.checked
                : (['checkbox', 'switch'].includes(roleOf(node).toLowerCase())
                  ? clean(node.getAttribute('aria-checked')).toLowerCase() === 'true'
                  : null);
              const current = stateOf(nativeControl(element)) ?? stateOf(element);
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
        "lingxiData", "lingxiNetwork", "lingxiRuntime", "lingxiDevice", "lingxiClipboard", "lingxiFiles",
        "lingxiCalendar", "lingxiContacts", "lingxiMedia", "lingxiLlm", "lingxiAgent",
        "lingxiBackground",
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

    /// Internal rather than private so `LocalAppsStoreTests` can pin the
    /// runtime-error ledger installed at document-start, matching
    /// `executionSource` above.
    static let bridgeSourceTemplate = #"""
    (() => {
      if (window.lingxi?.v2) return;
      // Criterion 6's evidence. A React ErrorBoundary swallows a render crash
      // into console.error and never reaches window.onerror, so the console
      // hook is the one that actually catches the shipped templates' failure
      // mode. Bounded on count AND per-field length: `result_json` is capped
      // at 256 KiB and FAILS rather than truncating.
      if (!window.__lingxiRuntimeErrors) {
        window.__lingxiRuntimeErrors = [];
        window.__lingxiRuntimeErrorsDropped = 0;
        const cap = 8;
        const trim = (v, n) => String(v == null ? '' : v).replace(/\s+/g, ' ').slice(0, n);
        const push = entry => {
          if (window.__lingxiRuntimeErrors.length >= cap) { window.__lingxiRuntimeErrorsDropped += 1; return; }
          window.__lingxiRuntimeErrors.push(entry);
        };
        window.addEventListener('error', e => push({
          kind: 'error', message: trim(e.message, 200), source: trim(e.filename, 120),
          line: e.lineno | 0, column: e.colno | 0, at_ms: Date.now(),
        }));
        window.addEventListener('unhandledrejection', e => push({
          kind: 'rejection',
          // Never expand an arbitrary rejection value — take a safe string only.
          message: trim(e.reason && e.reason.message ? e.reason.message : e.reason, 200),
          source: '', line: 0, column: 0, at_ms: Date.now(),
        }));
        const nativeConsoleError = console.error.bind(console);
        console.error = function () {
          try {
            push({ kind: 'console', message: trim(Array.from(arguments).map(a => (a && a.message) ? a.message : a).join(' '), 200),
                   source: '', line: 0, column: 0, at_ms: Date.now() });
          } catch (ignored) { /* never let the ledger break the page */ }
          return nativeConsoleError.apply(console, arguments);
        };
      }
      const installCsp = () => {
        if (!document.head || document.head.querySelector('meta[data-lingxi-csp]')) return false;
        const meta = document.createElement('meta');
        meta.httpEquiv = 'Content-Security-Policy';
        meta.dataset.lingxiCsp = 'v2';
        meta.content = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
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
        if (url.origin !== location.origin) throw new TypeError('External network access must use window.lingxi.v2.network');
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
      const streamListeners = new Map();
      const emitStream = frame => {
        const channel = pending.get(frame.requestId)?.channel;
        for (const [listener, listenerChannel] of streamListeners) {
          if (listenerChannel !== channel) continue;
          try { listener(frame); } catch (_) { /* app listener isolation */ }
        }
      };
      const request = (namespace, operation, payload = {}, channel = null) => new Promise((resolve, reject) => {
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
        const byteLimit = namespace === 'Llm' && (operation === 'chat' || operation === 'stream') ? 8388608 :
          (namespace === 'Files' && (operation === 'read' || operation === 'write') ? 4194304 : 65536);
        if (new TextEncoder().encode(serialized).byteLength > byteLimit) {
          const error = new Error(`Bridge request exceeds ${byteLimit} bytes`);
          error.code = 'request_too_large';
          reject(error);
          return;
        }
        pending.set(requestId, { resolve, reject, channel });
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
          share: (payload = {}) => request('Device', 'share', payload),
          synthesizeSpeech: (payload = {}) => request('Device', 'synthesizeSpeech', payload),
          status: () => request('Device', 'status', {}),
          haptics: style => request('Device', 'haptics', { style }),
          deepLink: url => request('Device', 'deepLink', { url }),
        }),
        clipboard: Object.freeze({
          getText: () => request('Clipboard', 'getText', {}),
          setText: text => request('Clipboard', 'setText', { text }),
        }),
        files: Object.freeze({
          read: payload => request('Files', 'read', payload),
          write: payload => request('Files', 'write', payload),
        }),
        calendar: Object.freeze({
          listEvents: payload => request('Calendar', 'listEvents', payload),
        }),
        contacts: Object.freeze({
          search: payload => request('Contacts', 'search', payload),
        }),
        media: Object.freeze({
          get: payload => request('Media', 'get', payload),
        }),
        llm: Object.freeze({
          chat: payload => request('Llm', 'chat', payload),
          stream: payload => request('Llm', 'stream', payload, 'llm'),
          onFrame: listener => {
            if (typeof listener !== 'function') throw new TypeError('LLM stream listener must be a function');
            streamListeners.set(listener, 'llm');
            return () => streamListeners.delete(listener);
          },
        }),
        agent: Object.freeze({
          post: payload => request('Agent', 'post', payload),
          sessions: Object.freeze({
            create: (payload = {}) => request('Agent', 'sessionCreate', payload),
            list: () => request('Agent', 'sessionList', {}),
            resume: (payload) => request('Agent', 'sessionResume', payload),
            close: (payload) => request('Agent', 'sessionClose', payload),
          }),
          send: payload => request('Agent', 'send', payload),
          stream: payload => request('Agent', 'stream', payload, 'agent'),
          cancel: payload => request('Agent', 'cancel', payload),
          onFrame: listener => {
            if (typeof listener !== 'function') throw new TypeError('Agent stream listener must be a function');
            streamListeners.set(listener, 'agent');
            return () => streamListeners.delete(listener);
          },
          profiles: Object.freeze({
            proposeUpdate: payload => request('Agent', 'profileProposeUpdate', payload),
          }),
        }),
        background: Object.freeze({
          schedule: payload => request('Background', 'schedule', payload),
          list: (payload = {}) => request('Background', 'list', payload),
          status: payload => request('Background', 'status', payload),
          cancel: payload => request('Background', 'cancel', payload),
          retry: payload => request('Background', 'retry', payload),
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
        value: Object.freeze({ v2: api, __resolve: resolveNative, __stream: emitStream }),
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
