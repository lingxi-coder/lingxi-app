import Foundation
import CryptoKit
import WebKit
import UIKit
import AVFoundation
import UserNotifications
import PhotosUI
import UniformTypeIdentifiers

@MainActor
final class UnifiedLocalAppBrowser {
    private unowned let service: UnifiedLocalApps
    private var sessions: [String: LocalAppBrowserSession] = [:]
    init(service: UnifiedLocalApps) { self.service = service }
    func open(_ id: String, external: @escaping (URL) -> Void) async throws -> WKWebView {
        if let session = sessions[id], session.isCurrent { session.activate(); return session.webView }
        close(id)
        let launch = try await service.launch(id)
        let release = try await service.release(id, revision: launch.run.revision)
        guard service.runs[id]?.runId == launch.run.runId else { throw LocalAppServiceFailure("RUN_CHANGED", "The application stopped while opening.") }
        let session = try LocalAppBrowserSession(service: service, launch: launch, capabilities: Set(release.manifest.capabilities ?? []), external: external)
        sessions[id] = session
        session.activate()
        return session.webView
    }
    func close(_ id: String) { sessions.removeValue(forKey: id)?.close() }
    func deactivate(_ id: String) { sessions[id]?.deactivate() }
    func clearData(_ id: String) async throws {
        close(id)
        let storeID = LocalAppBrowserIdentity.storeID(id)
        // A dismissed WebView may still retain its data store. Clear the store's
        // contents and await completion instead of removing an in-use profile.
        let store = WKWebsiteDataStore(forIdentifier: storeID)
        await store.removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast)
    }
}

@MainActor
private final class LocalAppBrowserSession: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandlerWithReply {
    private unowned let service: UnifiedLocalApps
    private let launch: LocalAppLaunch
    private let origin: URL
    private let capabilities: Set<String>
    private let external: (URL) -> Void
    private var closed = false
    private var activePage = false
    private var picker: LocalAppMediaPicker?
    private var recording: LocalAppRecording?
    private var permissionContinuation: CheckedContinuation<Bool, Never>?
    private weak var permissionAlert: UIAlertController?
    let webView: WKWebView
    var isCurrent: Bool { !closed && service.runs[launch.run.appId]?.runId == launch.run.runId && service.runs[launch.run.appId]?.phase == "running" }
    func activate() { activePage = true }
    func deactivate() {
        if activePage {
            let client = service.client, appID = launch.run.appId, invocationID = launch.run.runId
            Task { try? await client.agentClosed(appID: appID, invocationID: invocationID) }
        }
        activePage = false
        picker?.cancel(); recording?.cancel()
        permissionAlert?.dismiss(animated: false)
        permissionContinuation?.resume(returning: false); permissionContinuation = nil
    }

    init(service: UnifiedLocalApps, launch: LocalAppLaunch, capabilities: Set<String>, external: @escaping (URL) -> Void) throws {
        guard let origin = URL(string: launch.origin), LocalAppBrowserIdentity.matches(launch.url, origin: origin), origin.port != nil else {
            throw LocalAppServiceFailure("INVALID_APP_ORIGIN", "The app gateway did not provide its registered loopback origin.")
        }
        self.service = service; self.launch = launch; self.origin = origin; self.capabilities = capabilities; self.external = external
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = WKWebsiteDataStore(forIdentifier: LocalAppBrowserIdentity.storeID(launch.run.appId))
        configuration.allowsInlineMediaPlayback = true
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        webView.navigationDelegate = self; webView.uiDelegate = self
        configuration.userContentController.addScriptMessageHandler(self, contentWorld: .page, name: "localApp")
        configuration.userContentController.addUserScript(WKUserScript(source: try Self.bridge(launch), injectionTime: .atDocumentStart, forMainFrameOnly: true))
        webView.load(URLRequest(url: launch.url))
    }
    func close() {
        let client = service.client, appID = launch.run.appId, invocationID = launch.run.runId
        Task { try? await client.agentClosed(appID: appID, invocationID: invocationID) }
        closed = true
        deactivate()
        permissionAlert?.dismiss(animated: false)
        permissionContinuation?.resume(returning: false); permissionContinuation = nil
        picker?.cancel(); picker = nil
        recording?.cancel(); recording = nil
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "localApp", contentWorld: .page)
        webView.stopLoading(); webView.navigationDelegate = nil; webView.uiDelegate = nil
        webView.loadHTMLString("", baseURL: nil); webView.removeFromSuperview()
    }
    private func authorize(_ capability: String, frame: WKFrameInfo) throws {
        guard isCurrent, frame.isMainFrame, LocalAppBrowserIdentity.matches(frame.request.url, origin: origin), LocalAppBrowserIdentity.matches(webView.url, origin: origin) else {
            throw LocalAppServiceFailure("STALE_APP_CALL", "The app invocation is no longer active.")
        }
        guard capability.isEmpty || (activePage && webView.window != nil && UIApplication.shared.applicationState == .active) else {
            throw LocalAppServiceFailure("INACTIVE_APP_PAGE", "Open this application page before using a device capability.")
        }
        guard capability.isEmpty || (LocalAppDevicePermissions.supported.contains(capability) && capabilities.contains(capability)) else { throw LocalAppServiceFailure("CAPABILITY_NOT_DECLARED", "This release or distribution channel does not provide \(capability).") }
        guard capability.isEmpty || service.devicePermissions.allows(capability, appID: launch.run.appId, digest: launch.run.releaseDigest) else {
            throw LocalAppServiceFailure("DEVICE_PERMISSION_REQUIRED", "Allow \(capability) for this app version in app details.")
        }
    }
    private func requestDevicePermission(_ capability: String, frame: WKFrameInfo) async throws {
        guard !capability.isEmpty else { return }
        guard isCurrent, activePage, webView.window != nil, UIApplication.shared.applicationState == .active,
              frame.isMainFrame, LocalAppBrowserIdentity.matches(frame.request.url, origin: origin),
              capabilities.contains(capability), LocalAppDevicePermissions.supported.contains(capability) else { throw LocalAppServiceFailure("CAPABILITY_NOT_DECLARED", "This app cannot request that device capability.") }
        if service.devicePermissions.allows(capability, appID: launch.run.appId, digest: launch.run.releaseDigest) { return }
        guard permissionContinuation == nil else { throw LocalAppServiceFailure("PERMISSION_BUSY", "Another device permission is awaiting your answer.") }
        let presenter = try presenter()
        let allowed = await withCheckedContinuation { continuation in
            permissionContinuation = continuation
            let alert = UIAlertController(title: "Allow \(capability)?", message: "\(service.record(launch.run.appId)?.name ?? "This app") is requesting device access. Your choice applies only to this exact app release.", preferredStyle: .alert)
            permissionAlert = alert
            let finish: (Bool) -> Void = { [weak self] value in
                guard let self, let pending = self.permissionContinuation else { return }
                self.permissionContinuation = nil; self.permissionAlert = nil; pending.resume(returning: value)
            }
            alert.addAction(UIAlertAction(title: "Don't Allow", style: .cancel) { _ in finish(false) })
            alert.addAction(UIAlertAction(title: "Allow for This Version", style: .default) { _ in finish(true) })
            presenter.present(alert, animated: true)
        }
        guard allowed, try await service.runIsCurrent(appID: launch.run.appId, runID: launch.run.runId),
              isCurrent, frame.isMainFrame, LocalAppBrowserIdentity.matches(webView.url, origin: origin) else {
            throw LocalAppServiceFailure("DEVICE_PERMISSION_DENIED", "Device access was not authorized for the current app invocation.")
        }
        service.devicePermissions.set(capability, allowed: true, appID: launch.run.appId, digest: launch.run.releaseDigest)
    }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage, replyHandler: @escaping (Any?, String?) -> Void) {
        guard let input = message.body as? [String: Any], let method = input["method"] as? String else { replyHandler(nil, "Invalid native request"); return }
        let args = input["args"] as? [String: Any] ?? [:]
        let frame = message.frameInfo
        Task { @MainActor in
            do {
                if ["agent.run", "agent.get", "agent.cancel"].contains(method) {
                    try authorize("", frame: frame)
                    guard capabilities.contains("agent"), try await service.runIsCurrent(appID: launch.run.appId, runID: launch.run.runId) else { throw LocalAppServiceFailure("AGENT_NOT_DECLARED", "This app invocation cannot call an Agent.") }
                    if method == "agent.run", !(activePage && webView.window != nil && UIApplication.shared.applicationState == .active) { throw LocalAppServiceFailure("INACTIVE_APP_PAGE", "Open this application before starting an Agent task.") }
                    let data = try JSONSerialization.data(withJSONObject: args)
                    guard data.count <= 24 * 1024 * 1024 else { throw LocalAppServiceFailure("AGENT_INPUT_TOO_LARGE", "Resize images before sending them to the Agent.") }
                    let result = try await service.client.agentPage(appID: launch.run.appId, invocationID: launch.run.runId, method: method, arguments: String(decoding: data, as: UTF8.self))
                    try authorize("", frame: frame)
                    guard try await service.runIsCurrent(appID: launch.run.appId, runID: launch.run.runId) else { throw LocalAppServiceFailure("STALE_APP_CALL", "The application has stopped.") }
                    let value = try JSONSerialization.jsonObject(with: JSONEncoder().encode(result), options: .fragmentsAllowed)
                    replyHandler(value, nil)
                    return
                }
                let capability = method.split(separator: ".").first.map(String.init) ?? ""
                let required = ["info", "serviceSocketUrl"].contains(method) ? "" : (method == "notify" ? "notifications" : capability)
                guard try await service.runIsCurrent(appID: launch.run.appId, runID: launch.run.runId) else { throw LocalAppServiceFailure("STALE_APP_CALL", "The application has stopped.") }
                try await requestDevicePermission(required, frame: frame)
                try authorize(required, frame: frame)
                let value: Any
                switch method {
                case "info":
                    let layout = try await service.layout(launch.run.appId)
                    value = ["id": launch.run.appId, "name": service.record(launch.run.appId)?.name ?? "Local App", "runId": launch.run.runId, "revision": launch.run.revision, "platform": "ios", "dataDirectory": try UnifiedLocalApps.guestDirectory(layout.dataPath)] as [String: Any]
                case "serviceSocketUrl":
                    guard let name = args["name"] as? String, launch.run.services.contains(where: { $0.name == name }),
                          let base = URL(string: "\(launch.origin)/__run/\(launch.run.runId)/__service/\(name)/") else { throw LocalAppServiceFailure("UNKNOWN_SERVICE", "The application service is not registered for this run.") }
                    let path = (args["path"] as? String ?? "/").drop(while: { $0 == "/" })
                    guard let url = URL(string: String(path), relativeTo: base)?.absoluteURL, url.absoluteString.hasPrefix(base.absoluteString) else { throw LocalAppServiceFailure("INVALID_SERVICE_PATH", "Service paths must stay inside their registered route.") }
                    var parts = URLComponents(url: url, resolvingAgainstBaseURL: false)!; parts.scheme = "ws"
                    value = parts.url!.absoluteString
                case "clipboard.read": value = UIPasteboard.general.string ?? ""
                case "clipboard.write":
                    guard let text = args["text"] as? String, text.utf8.count <= 1024 * 1024 else { throw LocalAppServiceFailure("INVALID_TEXT", "Clipboard text is missing or too large.") }
                    UIPasteboard.general.string = text; value = true
                case "notifications.request":
                    value = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
                case "notifications.show", "notify":
                    let content = UNMutableNotificationContent()
                    content.title = String((args["title"] as? String ?? "Local app").prefix(160))
                    content.body = String((args["body"] as? String ?? "").prefix(2000))
                    try await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: "local-app:\(launch.run.appId):\(UUID())", content: content, trigger: nil))
                    value = true
                case "files.pick", "photos.pick", "camera.capture":
                    guard picker == nil else { throw LocalAppServiceFailure("PICKER_BUSY", "A file or camera operation is already open.") }
                    let activePicker = LocalAppMediaPicker(); picker = activePicker
                    defer { picker = nil }
                    value = try await activePicker.pick(method: method, presenter: presenter())
                case "microphone.record":
                    guard recording == nil else { throw LocalAppServiceFailure("MICROPHONE_BUSY", "A recording request is already open.") }
                    let capture = LocalAppRecording(appID: launch.run.appId, runID: launch.run.runId)
                    recording = capture
                    defer { recording = nil }
                    value = try await capture.record(presenter: presenter()) { [self] in
                        guard try await service.runIsCurrent(appID: launch.run.appId, runID: launch.run.runId) else { throw LocalAppServiceFailure("STALE_APP_CALL", "The application stopped.") }
                        try authorize("microphone", frame: frame)
                    }
                case "share.text":
                    guard let text = args["text"] as? String, text.utf8.count <= 1024 * 1024 else { throw LocalAppServiceFailure("INVALID_TEXT", "Share text is missing or too large.") }
                    let activity = UIActivityViewController(activityItems: [text], applicationActivities: nil)
                    let presenter = try presenter()
                    activity.popoverPresentationController?.sourceView = presenter.view
                    presenter.present(activity, animated: true)
                    value = true
                default: throw LocalAppServiceFailure("UNKNOWN_CAPABILITY_METHOD", "Unsupported native operation.")
                }
                guard try await service.runIsCurrent(appID: launch.run.appId, runID: launch.run.runId) else { throw LocalAppServiceFailure("STALE_APP_CALL", "The application has stopped.") }
                try authorize(required, frame: frame)
                replyHandler(value, nil)
            } catch { replyHandler(nil, error.localizedDescription) }
        }
    }
    private func presenter() throws -> UIViewController {
        guard let scene = webView.window?.windowScene, let root = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController else {
            throw LocalAppServiceFailure("PRESENTATION_UNAVAILABLE", "Open the app before using a device capability.")
        }
        var top = root
        while let shown = top.presentedViewController, !shown.isBeingDismissed { top = shown }
        return top
    }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard isCurrent, let url = action.request.url else { decisionHandler(.cancel); return }
        if LocalAppBrowserIdentity.matches(url, origin: origin) { decisionHandler(.allow); return }
        decisionHandler(.cancel)
        if action.targetFrame?.isMainFrame != false, ["https", "mailto", "tel"].contains(url.scheme ?? "") { external(url) }
    }
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if isCurrent, let url = action.request.url, ["https", "mailto", "tel"].contains(url.scheme ?? "") { external(url) }
        return nil
    }
    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin, initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType, decisionHandler: @escaping (WKPermissionDecision) -> Void) {
        // Device capture uses explicit native calls. Microphone recording must
        // share the audio lease with chat and stop on revocation/backgrounding.
        decisionHandler(.deny)
    }
    private static func bridge(_ launch: LocalAppLaunch) throws -> String {
        let base = String(decoding: try JSONEncoder().encode(launch.origin + "/__run/" + launch.run.runId + "/__service/"), as: UTF8.self)
        let names = String(decoding: try JSONEncoder().encode(launch.run.services.map(\.name)), as: UTF8.self)
        return """
    (() => { const base = \(base), names = new Set(\(names));
    const serviceUrl = (name, path='/') => {
      if (!names.has(name)) throw new Error('Unknown service for this app run');
      const root = base + encodeURIComponent(name) + '/';
      const url = new URL(String(path).replace(/^\\/+/, ''), root);
      if (!url.href.startsWith(root)) throw new Error('Service path escaped its registered route');
      return url.href;
    };
    const invoke = (method, args = {}) => window.webkit.messageHandlers.localApp.postMessage({method,args});
    const agent=Object.freeze({run:input=>invoke('agent.run',input),get:runId=>invoke('agent.get',{runId}),cancel:runId=>invoke('agent.cancel',{runId}),subscribe:(runId,listener)=>{
      if(typeof listener!=='function')throw new TypeError('A listener is required');let active=true,timer,last=0;
      const poll=async()=>{try{const run=await invoke('agent.get',{runId});if(active&&run.sequence>last){last=run.sequence;listener({type:'state',runId,sequence:run.sequence,time:run.updatedAt,run});}if(['succeeded','failed','cancelled','interrupted'].includes(run.state))active=false;}catch(error){if(active)console.warn('Agent subscription:',error.message);}if(active)timer=setTimeout(poll,750);};poll();return()=>{active=false;clearTimeout(timer);};
    }});
    Object.defineProperty(window, 'localApp', {value: Object.freeze({
      info:()=>invoke('info'), agent, notify:body=>invoke('notify',{body}), serviceUrl,
      serviceSocketUrl:(name,path='/')=>invoke('serviceSocketUrl',{name,path}),
      clipboard: Object.freeze({read:()=>invoke('clipboard.read'),write:text=>invoke('clipboard.write',{text})}),
      files: Object.freeze({pick:()=>invoke('files.pick')}), photos: Object.freeze({pick:()=>invoke('photos.pick')}),
      camera: Object.freeze({capture:()=>invoke('camera.capture')}), share: Object.freeze({text:text=>invoke('share.text',{text})}),
      microphone: Object.freeze({record:()=>invoke('microphone.record')}),
      notifications: Object.freeze({request:()=>invoke('notifications.request'),show:(title,body)=>invoke('notifications.show',{title,body})})
    }), configurable:false, writable:false}); })();
    """
    }
}

@MainActor
private final class LocalAppMediaPicker: NSObject, PHPickerViewControllerDelegate, UIImagePickerControllerDelegate, UINavigationControllerDelegate, UIDocumentPickerDelegate {
    private var continuation: CheckedContinuation<[String: String], Error>?
    private weak var controller: UIViewController?
    func pick(method: String, presenter: UIViewController) async throws -> [String: String] {
        try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation
            let picker: UIViewController
            if method == "photos.pick" {
                var config = PHPickerConfiguration(); config.filter = .images; config.selectionLimit = 1
                let photo = PHPickerViewController(configuration: config); photo.delegate = self; picker = photo
            } else if method == "camera.capture" {
                guard UIImagePickerController.isSourceTypeAvailable(.camera) else { finish(.failure(LocalAppServiceFailure("CAMERA_UNAVAILABLE", "Camera is unavailable."))); return }
                let camera = UIImagePickerController(); camera.sourceType = .camera; camera.delegate = self; picker = camera
            } else {
                let document = UIDocumentPickerViewController(forOpeningContentTypes: [.data], asCopy: true)
                document.allowsMultipleSelection = false; document.delegate = self; picker = document
            }
            controller = picker; presenter.present(picker, animated: true)
        }
    }
    func cancel() { finish(.failure(CancellationError())) }
    private func finish(_ result: Result<[String: String], Error>) {
        guard let pending = continuation else { return }; continuation = nil
        controller?.dismiss(animated: true); controller = nil; pending.resume(with: result)
    }
    private func value(_ data: Data, name: String, type: String) throws -> [String: String] {
        guard data.count <= 16 * 1024 * 1024 else { throw LocalAppServiceFailure("FILE_TOO_LARGE", "Select a file smaller than 16 MB.") }
        return ["name": name, "mimeType": type, "base64": data.base64EncodedString()]
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { cancel() }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first else { cancel(); return }
        do {
            let scoped = url.startAccessingSecurityScopedResource(); defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            let attributes = try url.resourceValues(forKeys: [.fileSizeKey, .contentTypeKey])
            guard (attributes.fileSize ?? Int.max) <= 16 * 1024 * 1024 else { throw LocalAppServiceFailure("FILE_TOO_LARGE", "Select a file smaller than 16 MB.") }
            finish(.success(try value(Data(contentsOf: url), name: url.lastPathComponent, type: attributes.contentType?.preferredMIMEType ?? "application/octet-stream")))
        } catch { finish(.failure(error)) }
    }
    func imagePickerControllerDidCancel(_ picker: UIImagePickerController) { cancel() }
    func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
        guard let image = info[.originalImage] as? UIImage, let data = image.jpegData(compressionQuality: 0.85) else { cancel(); return }
        do { finish(.success(try value(data, name: "photo.jpg", type: "image/jpeg"))) } catch { finish(.failure(error)) }
    }
    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        guard let provider = results.first?.itemProvider else { cancel(); return }
        provider.loadDataRepresentation(forTypeIdentifier: UTType.image.identifier) { [weak self] data, error in
            Task { @MainActor in
                guard let self else { return }
                do {
                    if let error { throw error }
                    guard let data else { throw LocalAppServiceFailure("PHOTO_UNAVAILABLE", "The selected photo could not be loaded.") }
                    self.finish(.success(try self.value(data, name: "photo", type: "image/*")))
                } catch { self.finish(.failure(error)) }
            }
        }
    }
}
