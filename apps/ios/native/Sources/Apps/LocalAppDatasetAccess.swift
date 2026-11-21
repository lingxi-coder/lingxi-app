import CryptoKit
import FirebaseAuth
import Foundation
import SwiftUI

struct LocalAppDatasetBinding: Codable, Equatable, Sendable {
    var dataset: String
    var grant: String
}

@MainActor
enum LocalAppDatasetAccess {
    private struct Proof: Decodable { let proof: String }
    private struct Intent: Decodable {
        struct Binding: Decodable {
            struct Reference: Decodable { let kind: String; let dataset: String? }
            let reference: Reference
            let via: String?
            let edit: Bool
        }
        let bindings: [String: Binding]
    }
    private final class NoRedirect: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
        func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                        newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
            completionHandler(nil)
        }
    }

    static var origin: URL? {
        guard let value = Bundle.main.object(forInfoDictionaryKey: "LingxiDatasetBrowserOrigin") as? String,
              let url = URL(string: value), url.scheme == "https", url.host != nil, url.user == nil,
              url.password == nil, url.query == nil, url.fragment == nil, url.path.isEmpty || url.path == "/" else { return nil }
        return url
    }

    private static func key(_ uid: String, _ app: String, _ digest: String) -> String {
        "lingxi.local-app.dataset." + digestOf([uid, app, digest].joined(separator: ":"))
    }

    private static func digestOf(_ string: String) -> String {
        SHA256.hash(data: Data(string.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    private static func install(_ uid: String, _ app: String) -> String {
        let defaults = UserDefaults.standard
        let storage = key(uid, app, "installation")
        if let value = defaults.string(forKey: storage) { return value }
        let value = UUID().uuidString.lowercased()
        defaults.set(value, forKey: storage)
        return value
    }

    static func saved(_ release: UnifiedAppRelease) -> [String: LocalAppDatasetBinding] {
        guard let uid = Auth.auth().currentUser?.uid,
              let data = UserDefaults.standard.data(forKey: key(uid, release.appId, release.digest)) else { return [:] }
        return (try? JSONDecoder().decode([String: LocalAppDatasetBinding].self, from: data)) ?? [:]
    }

    static func savedIntent(_ release: UnifiedAppRelease, expectedUID: String) async throws -> [String: LocalAppDatasetBinding] {
        guard let origin, let user = Auth.auth().currentUser, user.uid == expectedUID else { return [:] }
        let data = try await command(origin, user: user, body: ["action": "getIntent", "application": release.appId, "release": release.digest])
        let intent = try JSONDecoder().decode(Intent.self, from: data)
        return Dictionary(uniqueKeysWithValues: intent.bindings.compactMap { alias, binding in
            guard let mode = release.manifest.dataBindings?[alias], binding.edit == (mode == "edit"),
                  binding.reference.kind == "dataset", let dataset = binding.reference.dataset else { return nil }
            return (alias, LocalAppDatasetBinding(dataset: dataset, grant: binding.via ?? ""))
        })
    }

    static func save(_ bindings: [String: LocalAppDatasetBinding], release: UnifiedAppRelease, expectedUID: String, sync: Bool) async throws {
        if sync {
            guard let origin, let user = Auth.auth().currentUser, user.uid == expectedUID else {
                throw unavailable("Dataset sign-in is unavailable.")
            }
            _ = try await command(origin, user: user, body: ["action": "saveIntent", "application": release.appId,
                "release": release.digest, "bindings": try encode(bindings, for: release)])
        }
        guard Auth.auth().currentUser?.uid == expectedUID else { throw unavailable("The signed-in account changed.") }
        UserDefaults.standard.set(try JSONEncoder().encode(bindings), forKey: key(expectedUID, release.appId, release.digest))
    }

    static func launch(_ release: UnifiedAppRelease, client: LocalAppServiceClient) async throws -> URL? {
        let declarations = release.manifest.dataBindings ?? [:]
        if declarations.isEmpty { return nil }
        guard let origin, let user = Auth.auth().currentUser else { throw unavailable("Sign in to open the Dataset connection.") }
        let uid = user.uid
        let installation = install(uid, release.appId)
        let data = try await command(origin, user: user, body: ["action": "launch", "install": [
            "application": release.appId, "installation": installation, "version": release.revision,
            "release": release.digest, "publisher": digestOf(installation),
        ], "bindings": try encode(saved(release), for: release)])
        let proof = try JSONDecoder().decode(Proof.self, from: data).proof
        guard Auth.auth().currentUser?.uid == uid else { throw unavailable("The signed-in account changed.") }
        return try await client.datasetBrowserLaunch(release.appId, revision: release.revision, origin: origin.absoluteString, proof: proof)
    }

    private static func encode(_ intents: [String: LocalAppDatasetBinding], for release: UnifiedAppRelease) throws -> [String: [String: Any]] {
        let bindings = try (release.manifest.dataBindings ?? [:]).map { alias, mode in
            guard let binding = intents[alias], !binding.dataset.isEmpty else {
                throw unavailable("Choose a Dataset for each declared app binding.")
            }
            var value: [String: Any] = ["reference": ["kind": "dataset", "dataset": binding.dataset], "edit": mode == "edit"]
            if !binding.grant.isEmpty { value["via"] = binding.grant }
            return (alias, value)
        }
        return Dictionary(uniqueKeysWithValues: bindings)
    }

    static func revoke(appID: String) async throws {
        guard let origin, let user = Auth.auth().currentUser else { return }
        let storage = key(user.uid, appID, "installation")
        guard let installation = UserDefaults.standard.string(forKey: storage) else { return }
        _ = try await command(origin, user: user, body: ["action": "revoke", "installation": installation])
    }

    private static func command(_ origin: URL, user: User, body: [String: Any]) async throws -> Data {
        let token = try await user.getIDToken()
        guard Auth.auth().currentUser?.uid == user.uid else { throw unavailable("The signed-in account changed.") }
        var request = URLRequest(url: URL(string: "/v1/dataset-browser/host", relativeTo: origin)!.absoluteURL, timeoutInterval: 15)
        request.httpMethod = "POST"
        request.setValue("Bearer " + token, forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await URLSession.shared.data(for: request, delegate: NoRedirect())
        guard Auth.auth().currentUser?.uid == user.uid else { throw unavailable("The signed-in account changed.") }
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw unavailable("Dataset access is unavailable or has been revoked.") }
        return data
    }

    private static func unavailable(_ message: String) -> LocalAppServiceFailure {
        LocalAppServiceFailure("DATASET_BROWSER_UNAVAILABLE", message)
    }
}

struct LocalAppDatasetConnectionSheet: View {
    let release: UnifiedAppRelease
    let onSave: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var bindings: [String: LocalAppDatasetBinding] = [:]
    @State private var owner: String?
    @State private var error: String?
    @State private var sync = false
    @State private var saving = false
    private var aliases: [String] { (release.manifest.dataBindings ?? [:]).keys.sorted() }

    var body: some View {
        NavigationStack {
            Form {
                ForEach(aliases, id: \.self) { alias in
                    Section(alias + (release.manifest.dataBindings?[alias] == "edit" ? " · 读写" : " · 读取")) {
                        TextField("数据集 ID", text: field(alias, \.dataset)).textInputAutocapitalization(.never).autocorrectionDisabled()
                        TextField("共享授权（可选）", text: field(alias, \.grant)).textInputAutocapitalization(.never).autocorrectionDisabled()
                    }
                }
                Toggle("同步绑定意向到我的设备", isOn: $sync)
                    .disabled(saving)
                if let error { Text(error).foregroundStyle(.red) }
            }
            .navigationTitle("连接数据集")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { dismiss() }.disabled(saving) }
                ToolbarItem(placement: .confirmationAction) {
                    Button("授权并打开") {
                        guard let owner else { return }
                        saving = true
                        Task {
                            defer { saving = false }
                            do {
                                try await LocalAppDatasetAccess.save(bindings, release: release, expectedUID: owner, sync: sync)
                                dismiss()
                                onSave()
                            } catch { self.error = error.localizedDescription }
                        }
                    }
                    .disabled(saving || owner == nil || aliases.contains { bindings[$0]?.dataset.isEmpty != false })
                }
            }
            .task(id: release.digest) {
                owner = Auth.auth().currentUser?.uid
                bindings = LocalAppDatasetAccess.saved(release)
                guard let owner, bindings.isEmpty,
                      let draft = try? await LocalAppDatasetAccess.savedIntent(release, expectedUID: owner),
                      !Task.isCancelled, bindings.isEmpty, !saving else { return }
                bindings = draft
            }
        }
        .interactiveDismissDisabled(saving)
    }

    private func field(_ alias: String, _ keyPath: WritableKeyPath<LocalAppDatasetBinding, String>) -> Binding<String> {
        Binding(get: { bindings[alias]?[keyPath: keyPath] ?? "" }, set: { value in
            var binding = bindings[alias] ?? LocalAppDatasetBinding(dataset: "", grant: "")
            binding[keyPath: keyPath] = value.trimmingCharacters(in: .whitespacesAndNewlines)
            bindings[alias] = binding
        })
    }
}
