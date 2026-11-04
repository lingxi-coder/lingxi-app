import FirebaseAuth
import FirebaseCore
import FirebaseFirestore
import Foundation
import Observation
import SwiftUI

/// This executable is a feature harness. No production plist, native engine,
/// credential session, FFI framework or owner tool capability is linked.
struct ProbeEnvironment: Sendable {
    let backend: URL
    let uid: String
    let authPort: Int
    let firestorePort: Int
    static let projectID = "demo-lingxi-teams-t0"

    init(_ values: [String: String]) throws {
        guard let raw = values["TEAM_TEST_BACKEND"], let parts = URLComponents(string: raw),
              parts.scheme == "http", parts.host == "127.0.0.1", let port = parts.port, (1...65535).contains(port),
              parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil,
              parts.path.isEmpty || parts.path == "/", let backend = parts.url,
              let uid = values["TEAM_TEST_UID"], !uid.isEmpty, uid.utf8.count <= 128,
              !uid.contains("/"), !uid.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { throw ProbeFailure.invalidEnvironment }
        func emulatorPort(_ key: String, fallback: Int) throws -> Int {
            guard let raw = values[key], !raw.isEmpty else { return fallback }
            guard let value = Int(raw), (1...65535).contains(value) else { throw ProbeFailure.invalidEnvironment }
            return value
        }
        self.backend = backend
        self.uid = uid
        authPort = try emulatorPort("TEAM_TEST_AUTH_PORT", fallback: 9098)
        firestorePort = try emulatorPort("TEAM_TEST_FIRESTORE_PORT", fallback: 8085)
    }
    var messagesURL: URL { backend.appendingPathComponent("v1/teams/messages") }
    var bootstrapURL: URL {
        var components = URLComponents(url: backend.appendingPathComponent("_acceptance/bootstrap"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "uid", value: uid)]
        return components.url!
    }
}
private enum ProbeFailure: Error { case invalidEnvironment, bootstrapFailed, wrongDemoIdentity }
enum ProbeErrorDiagnostics {
    static func summary(_ error: Error) -> String {
        var entries: [String] = []
        var current: NSError? = error as NSError
        var seen: Set<ObjectIdentifier> = []
        while let failure = current, entries.count < 8, seen.insert(ObjectIdentifier(failure)).inserted {
            entries.append("\(failure.domain) / \(failure.code)")
            if let status = keychainStatus(failure) { entries.append("\(NSOSStatusErrorDomain) / \(status)") }
            current = failure.userInfo[NSUnderlyingErrorKey] as? NSError
        }
        return entries.joined(separator: " -> ")
    }
    private static func keychainStatus(_ error: NSError) -> Int? {
        // Firebase 12.19.2 AuthErrorUtils.keychainError stores OSStatus only in
        // this fixed reason format. Extract its number; never log the reason.
        guard error.domain == "FIRAuthErrorDomain", error.code == 17995,
              let reason = error.userInfo[NSLocalizedFailureReasonErrorKey] as? String,
              let expression = try? NSRegularExpression(pattern: "^SecItem(?:CopyMatching|Add|Update|Delete) \\((-?[0-9]+)\\) "),
              let match = expression.firstMatch(in: reason, range: NSRange(reason.startIndex..., in: reason)),
              let range = Range(match.range(at: 1), in: reason) else { return nil }
        return Int(reason[range])
    }
}
private struct ProbeBootstrap: Decodable { let customToken: String; let detail: TeamDetail }
private final class ProbeNoRedirects: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}

@MainActor @Observable
private final class ProbeModel {
    var detail: TeamDetail?
    var messaging: TeamMessagingModel?
    var uid = ""
    var error = ""
    var closed = false
    private var started = false

    func start() async {
        guard !started else { return }
        started = true
        do {
            let environment = try ProbeEnvironment(ProcessInfo.processInfo.environment)
            let options = FirebaseOptions(googleAppID: "1:123456789:ios:0123456789abcdef", gcmSenderID: "123456789")
            options.projectID = ProbeEnvironment.projectID
            options.apiKey = "AIzaSyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            FirebaseConfiguration.shared.setLoggerLevel(.error)
            FirebaseApp.configure(options: options)
            Auth.auth().useEmulator(withHost: "127.0.0.1", port: environment.authPort)
            let firestore = Firestore.firestore()
            let settings = firestore.settings
            settings.host = "127.0.0.1:\(environment.firestorePort)"
            settings.isSSLEnabled = false
            settings.cacheSettings = MemoryCacheSettings()
            firestore.settings = settings

            let session = URLSession(configuration: .ephemeral, delegate: ProbeNoRedirects(), delegateQueue: nil)
            defer { session.invalidateAndCancel() }
            var request = URLRequest(url: environment.bootstrapURL)
            request.timeoutInterval = 20
            request.cachePolicy = .reloadIgnoringLocalCacheData
            let (data, response) = try await session.data(for: request)
            guard let response = response as? HTTPURLResponse, response.statusCode == 200 else { throw ProbeFailure.bootstrapFailed }
            let fixture = try JSONDecoder().decode(ProbeBootstrap.self, from: data)
            let result = try await Auth.auth().signIn(withCustomToken: fixture.customToken)
            guard result.user.uid == environment.uid,
                  fixture.detail.members.contains(where: { $0.uid == environment.uid && $0.generation == fixture.detail.team.generation })
            else { try? Auth.auth().signOut(); throw ProbeFailure.wrongDemoIdentity }
            uid = environment.uid
            messaging = TeamMessagingModel(apiRoot: environment.messagesURL)
            detail = fixture.detail
            // Synthetic UID only. Never print the custom token or SDK session.
            print("TEAMS_PROBE_READY uid=\(uid) project=\(ProbeEnvironment.projectID)")
        } catch {
            let summary = ProbeErrorDiagnostics.summary(error)
            self.error = "Demo bootstrap failed: \(summary)"
            print("TEAMS_PROBE_FAILED errors=\(summary)")
        }
    }
}

@main @MainActor
struct TeamsT3Probe: App {
    @State private var model = ProbeModel()
    var body: some Scene {
        WindowGroup {
            VStack(spacing: 0) {
                Text("Teams feature harness · emulators only").font(.caption).foregroundStyle(.secondary)
                if let detail = model.detail, let messaging = model.messaging, !model.closed {
                    Text("Demo account: \(model.uid)").font(.caption2).accessibilityIdentifier("teams.probe.ready")
                    NavigationStack {
                        TeamMessagesPage(uid: model.uid, detail: detail, onClose: { model.closed = true }, model: messaging)
                    }
                } else if model.closed {
                    Text("Teams feature harness closed").accessibilityIdentifier("teams.probe.closed")
                } else if !model.error.isEmpty {
                    Text(model.error).accessibilityIdentifier("teams.probe.error")
                } else {
                    ProgressView("Connecting to local demo fixture")
                }
            }.task { await model.start() }
        }
    }
}
