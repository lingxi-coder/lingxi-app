import Foundation
import Observation
import CryptoKit
import ZIPFoundation
import FirebaseAuth

struct LocalAppMarketPublication: Identifiable, Sendable {
    var id: String { source.digest }
    let source: LocalAppSourceArchive
    let name: String
    let summary: String
    let uid: String
}

@MainActor
@Observable
final class UnifiedAppMarket {
    private(set) var apps: [MarketApp] = []
    private(set) var error: String?
    private(set) var loading = false
    private(set) var progress: Double?
    private(set) var hasNextPage = false
    private(set) var publishStatus: String?
    private(set) var publishing = false
    private(set) var installCommitting = false
    @ObservationIgnored private var installTask: Task<UnifiedAppRecord, Error>?
    private var cursor: String?
    private var query = ""
    private var generation = UUID()
    private let library: UnifiedLocalApps
    private let api: MarketAPI?
    init(library: UnifiedLocalApps, baseURL: URL? = nil) {
        self.library = library
        struct ProductConfig: Decodable { let localAppMarketApiRoot: String }
        let configured = Bundle.main.url(forResource: "product-config", withExtension: "json")
            .flatMap { try? Data(contentsOf: $0) }
            .flatMap { try? JSONDecoder().decode(ProductConfig.self, from: $0) }
            .flatMap { URL(string: $0.localAppMarketApiRoot) }
        let endpoint = baseURL ?? configured
        api = endpoint.flatMap { try? MarketAPI(baseURL: $0) }
    }
    private func requireAPI() throws -> MarketAPI {
        guard let api else { throw MarketFailure("MARKET_NOT_CONFIGURED", "The app market endpoint is unavailable.") }; return api
    }
    var managementURL: URL? {
        guard let endpoint = api?.baseURL, var parts = URLComponents(url: endpoint, resolvingAgainstBaseURL: false) else { return nil }
        parts.path = "/mine"; parts.query = nil; parts.fragment = nil
        return parts.url
    }
    func search(_ value: String) async {
        query = value.trimmingCharacters(in: .whitespacesAndNewlines); generation = UUID(); cursor = nil; apps = []; hasNextPage = false
        let current = generation
        loading = true; error = nil
        defer { if current == generation { loading = false } }
        do {
            let api = try requireAPI(); try await api.capabilities()
            let page = try await api.catalog(query:query)
            guard current == generation else { return }; apps = page.data; cursor = page.pagination.nextCursor; hasNextPage = cursor != nil
        } catch { if current == generation { self.error = error.localizedDescription } }
    }
    func nextPage() async {
        guard !loading, let cursor else { return }; loading = true
        let current = generation
        defer { if current == generation { loading = false } }
        do {
            let page = try await requireAPI().catalog(query:query,cursor:cursor)
            guard current == generation else { return }; var seen = Set(apps.map(\.id)); apps += page.data.filter { seen.insert($0.id).inserted }
            self.cursor = page.pagination.nextCursor; hasNextPage = self.cursor != nil
        } catch { self.error = error.localizedDescription }
    }
    static var targets: [UnifiedAppManifest.Target] {
        let supported = ["camera", "photos", "files", "microphone", "clipboard", "share", "network", "agent"]
        let runtimeVersion: String? = {
            guard let url = Bundle.main.url(forResource: "mobile-linux-native-pins", withExtension: "json"),
                  let data = try? Data(contentsOf: url),
                  let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
            return value["sdk_version"] as? String
        }()
        #if LINGXI_FULL
        return [.init(platform:"ios",environment:"web",architecture:"aarch64",runtimeVersion:runtimeVersion,channels:["full"],capabilities:supported),
                .init(platform:"ios",environment:"mobile-linux",architecture:"aarch64",runtimeVersion:runtimeVersion,channels:["full"],capabilities:supported + ["process", "background"])]
        #else
        return [.init(platform:"ios",environment:"web",architecture:"aarch64",runtimeVersion:runtimeVersion,channels:["store"],capabilities:supported)]
        #endif
    }
    func detail(_ id: String) async throws -> MarketApp { try await requireAPI().detail(id) }
    func distribution(_ id: String) async throws -> MarketDistribution {
        let api = try requireAPI()
        var compatible: [MarketDistribution] = []
        for target in Self.targets {
            do { compatible.append(try await api.distribution(id,target:target)) }
            catch let error as MarketFailure where error.code == "NO_COMPATIBLE_RELEASE" { continue }
        }
        guard let result = compatible.max(by: { $0.sequence < $1.sequence }) else { throw MarketFailure("NO_COMPATIBLE_RELEASE", "No release supports this device and distribution channel.") }
        return result
    }
    func cancelInstall() { if !installCommitting { installTask?.cancel() } }
    func install(_ distribution: MarketDistribution, approved: Bool, remix: Bool = false) async throws -> UnifiedAppRecord {
        guard installTask == nil else { throw MarketFailure("INSTALL_IN_PROGRESS", "Wait for the current app installation to finish.") }
        let task = Task { try await performInstall(distribution, approved: approved, remix: remix) }
        installTask = task
        defer { installTask = nil; installCommitting = false }
        return try await task.value
    }
    private func performInstall(_ distribution: MarketDistribution, approved: Bool, remix: Bool) async throws -> UnifiedAppRecord {
        guard !distribution.executionConsentRequired || approved else { throw MarketFailure("EXECUTION_CONSENT_REQUIRED", "Review the exact release before installing executable code.") }
        guard !remix || distribution.manifest.remix?.allowed == true else { throw MarketFailure("REMIX_NOT_ALLOWED", "The publisher has not granted modification permission.") }
        await library.refresh()
        let previous = library.records.first { $0.marketOrigin?.appId == distribution.appId && $0.marketOrigin?.remixedFrom == nil }
        if !remix, let previous {
            if previous.marketOrigin?.releaseId == distribution.versionId { return previous }
            let candidate = try await library.release(previous.appId, revision: previous.latestRevision)
            if candidate.marketOrigin?.releaseId == distribution.versionId {
                try Task.checkCancellation(); installCommitting = true
                if approved { try await library.grant(previous.appId, digest: candidate.digest) }
                try await library.action("start", id: previous.appId, extra: ["revision": .number(Double(candidate.revision))])
                await library.refresh()
                guard let current = library.record(previous.appId) else { throw MarketFailure("APP_REMOVED") }
                return current
            }
        }
        let staging = FileManager.default.temporaryDirectory.appendingPathComponent("local-app-market-" + UUID().uuidString, isDirectory:true)
        try FileManager.default.createDirectory(at:staging,withIntermediateDirectories:true)
        defer { try? FileManager.default.removeItem(at:staging); progress = nil }
        progress = 0
        let archive = staging.appendingPathComponent("source.zip")
        try await requireAPI().download(distribution.artifact,to:archive) { [weak self] value in await self?.setProgress(value) }
        let content = staging.appendingPathComponent("source",isDirectory:true)
        try await Task.detached(priority: .userInitiated) {
            let data = try Data(contentsOf:archive)
            guard Self.digest(data) == distribution.artifact.sha256.lowercased() else { throw MarketFailure("HASH_MISMATCH") }
            try FileManager.default.createDirectory(at:content,withIntermediateDirectories:true)
            try MarketPackageFiles.extract(archive,to:content)
            try Self.verify(content,distribution:distribution)
        }.value
        try Task.checkCancellation()
        var source = content
        var authoring: LocalAppAuthoring?
        if remix {
            let guest = try UnifiedLocalApps.newAuthoringDirectory(name:distribution.manifest.name)
            source = try UnifiedLocalApps.hostSourceDirectory(guest)
            for child in try FileManager.default.contentsOfDirectory(at:content,includingPropertiesForKeys:nil) { try FileManager.default.copyItem(at:child,to:source.appendingPathComponent(child.lastPathComponent)) }
            authoring = .init(sourceDirectory:guest)
        }
        let origin = LocalAppMarketOrigin(appId:distribution.appId,releaseId:distribution.versionId,digest:distribution.releaseDigest,remixedFrom:remix ? distribution.appId : nil)
        var fields: [String: LocalAppJSON] = ["op": .string(previous != nil && !remix ? "publish" : "import"), "sourcePath": .string(source.path), "marketOrigin": try .encode(origin)]
        if let authoring { fields["authoring"] = try .encode(authoring) }
        if let previous, !remix { fields["appId"] = .string(previous.appId); fields["expectedRevision"] = .number(Double(previous.latestRevision)) }
        try Task.checkCancellation()
        installCommitting = true
        let result: LocalAppPublishResult = try await library.client.request(fields)
        // Consent binds the locally verified release, before any install/build/start.
        if approved { try await library.grant(result.record.appId,digest:result.release.digest) }
        if previous != nil, !remix {
            do { try await library.action("start", id: result.record.appId, extra: ["revision": .number(Double(result.release.revision))]) }
            catch { await library.refresh(); throw error }
        }
        await library.refresh()
        return library.record(result.record.appId) ?? result.record
    }
    private func setProgress(_ value: Double) { progress = value }
    nonisolated static func digest(_ data: Data) -> String { SHA256.hash(data:data).map { String(format:"%02x",$0) }.joined() }
    nonisolated static func verify(_ content: URL, distribution: MarketDistribution) throws {
        let expected = Set(distribution.files.map(\.path))
        guard expected.count == distribution.files.count, expected.contains("lingxi-app.json") else { throw MarketFailure("INVALID_FILE_MANIFEST") }
        let files = FileManager.default.enumerator(at:content,includingPropertiesForKeys:[.isRegularFileKey,.isSymbolicLinkKey])!
        var actual = Set<String>()
        for case let file as URL in files {
            let attributes = try file.resourceValues(forKeys:[.isRegularFileKey,.isSymbolicLinkKey])
            guard attributes.isSymbolicLink != true else { throw MarketFailure("SYMLINK_REJECTED") }
            if attributes.isRegularFile == true { actual.insert(String(file.path.dropFirst(content.path.count + 1))) }
        }
        guard actual == expected else { throw MarketFailure("PACKAGE_FILE_SET_MISMATCH") }
        for file in distribution.files {
            let path = try MarketPackageFiles.safePath(file.path); let data = try Data(contentsOf:content.appendingPathComponent(path))
            guard data.count == file.sizeBytes, digest(data) == file.sha256.lowercased() else { throw MarketFailure("FILE_HASH_MISMATCH") }
            try FileManager.default.setAttributes([.posixPermissions: file.executable ? 0o700 : 0o600], ofItemAtPath: content.appendingPathComponent(path).path)
        }
        for dependency in distribution.dependencies {
            guard distribution.files.contains(where: { $0.path == dependency.path && $0.sha256 == dependency.sha256 && $0.sizeBytes == dependency.sizeBytes && $0.executable == dependency.executable }) else { throw MarketFailure("DEPENDENCY_LOCK_MISMATCH") }
        }
        let manifest = try JSONDecoder().decode(UnifiedAppManifest.self,from:Data(contentsOf:content.appendingPathComponent("lingxi-app.json")))
        guard manifest == distribution.manifest else { throw MarketFailure("MANIFEST_MISMATCH") }
    }
    private func publicationReceipts() throws -> LocalAppPublicationReceipts {
        try LocalAppPublicationReceipts(directory: URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
            .appendingPathComponent("local-app-market-publishing", isDirectory: true))
    }
    func preparePublication(_ app: UnifiedAppRecord) async throws -> LocalAppMarketPublication {
        guard let user = Auth.auth().currentUser else { throw MarketFailure("SIGN_IN_REQUIRED", "Sign in to publish to the market.") }
        let currentRelease = try await library.release(app.appId, revision: app.currentRevision)
        let releaseName = currentRelease.manifest.name
        let releaseSummary = currentRelease.manifest.summary ?? ""
        guard !releaseSummary.isEmpty else { throw MarketFailure("SUMMARY_REQUIRED", "Add an app summary and publish a local release before publishing to the market.") }
        let exported = try await library.sourceArchive(app.appId, revision: app.currentRevision)
        guard exported.digest == currentRelease.digest, Auth.auth().currentUser?.uid == user.uid else { throw MarketFailure("PUBLICATION_CHANGED", "The app or author account changed. Review the source again.") }
        let receipts = try publicationReceipts()
        let releaseKey = "local-app-market-publisher:\(user.uid):\(app.appId):\(exported.digest)"
        let cache = URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
            .appendingPathComponent("local-app-market-publishing", isDirectory: true)
            .appendingPathComponent(Self.digest(Data(releaseKey.utf8)), isDirectory: true)
        let archive = cache.appendingPathComponent("source.zip")
        let bytes = try receipts.preparedArchive(archive, releaseKey: releaseKey, proposed: exported.bytes)
        let source = LocalAppSourceArchive(appID: exported.appID, revision: exported.revision, digest: exported.digest, files: exported.files, bytes: bytes)
        return .init(source: source, name: releaseName, summary: releaseSummary, uid: user.uid)
    }
    func publish(_ prepared: LocalAppMarketPublication) async throws {
        guard !publishing else { throw MarketFailure("PUBLISH_IN_PROGRESS", "A publication is already in progress.") }
        publishing = true; defer { publishing = false }
        guard let user = Auth.auth().currentUser, user.uid == prepared.uid else { throw MarketFailure("ACCOUNT_CHANGED", "Review the source again with the current author account.") }
        let source = prepared.source
        await library.refresh()
        guard let app = library.record(source.appID), app.currentRevision == source.revision,
              try await library.release(app.appId, revision: source.revision).digest == source.digest else {
            throw MarketFailure("PUBLICATION_CHANGED", "The app version changed. Review the source again.")
        }
        let releaseName = prepared.name, releaseSummary = prepared.summary
        let uid = user.uid; let token = try await user.getIDToken()
        let api = try requireAPI(); try await api.capabilities()
        let receipts = try publicationReceipts()
        let key = "local-app-market-publisher:\(uid):\(app.appId)"
        let releaseKey = key + ":\(source.digest)"
        let cache = URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
            .appendingPathComponent("local-app-market-publishing", isDirectory: true)
            .appendingPathComponent(Self.digest(Data(releaseKey.utf8)), isDirectory: true)
        let archive = cache.appendingPathComponent("source.zip")
        let bytes = try receipts.preparedArchive(archive, releaseKey: releaseKey, proposed: source.bytes)
        guard bytes == source.bytes else { throw MarketFailure("PUBLISH_ARCHIVE_CHANGED", "Retry the reviewed publication or create a new local release.") }
        var versionKey = releaseKey + (try receipts.get(releaseKey + ":attempt").map { ":" + $0 } ?? "")
        func mutationKey(_ step: String) throws -> String {
            try receipts.mutationKey(versionKey + ":" + step)
        }
        guard bytes.count <= 25 * 1024 * 1024 else { throw MarketFailure("UPLOAD_TOO_LARGE") }
        func checkAccount() throws { guard Auth.auth().currentUser?.uid == uid else { throw MarketFailure("ACCOUNT_CHANGED") } }
        try checkAccount()
        var marketID = try receipts.get(key)
        if marketID == nil {
            let proposed: [String: LocalAppJSON] = ["name": .string(String(releaseName.prefix(60))), "summary": .string(String(releaseSummary.prefix(160))), "category": .string("other")]
            let original = try receipts.fixedValue(key + ":createBody", proposed: String(decoding: JSONEncoder().encode(proposed), as: UTF8.self))
            let body = try JSONDecoder().decode([String: LocalAppJSON].self, from: Data(original.utf8))
            let created = try await api.publish("apps", body: body, bearer: token, idempotencyKey: receipts.mutationKey(key + ":createListing"))
            guard case .object(let result) = created, let id = result["id"]?.text else { throw MarketFailure("INVALID_CREATE_RESPONSE") }
            marketID = id; try receipts.set(key, id)
        }
        try checkAccount()
        var versionID = try receipts.get(versionKey + ":versionID")
        if let previous = versionID {
            let status = try await api.ownedVersion(previous, bearer: token)
            if case .object(let value) = status, value["status"]?.text == "failed",
               case .object(let error)? = value["error"], error["code"]?.text == "UPLOAD_EXPIRED" {
                let attempt = UUID().uuidString
                try receipts.set(releaseKey + ":attempt", attempt)
                versionKey = releaseKey + ":" + attempt
                versionID = nil
            }
        }
        if versionID == nil {
            try checkAccount()
            let proposed: [String: LocalAppJSON] = ["format": .string("zip"), "filename": .string("source.zip"), "sizeBytes": .number(Double(bytes.count)), "sha256": .string(Self.digest(bytes)), "notes": .string("Local release \(app.currentRevision)")]
            let original = try receipts.fixedValue(versionKey + ":versionBody", proposed: String(decoding: JSONEncoder().encode(proposed), as: UTF8.self))
            let body = try JSONDecoder().decode([String: LocalAppJSON].self, from: Data(original.utf8))
            let version = try await api.publish("apps/\(marketID!)/versions",body:body,bearer:token,idempotencyKey:mutationKey("createVersion"))
            guard case .object(let result) = version, let id = result["id"]?.text else { throw MarketFailure("INVALID_VERSION_RESPONSE") }
            versionID = id; try receipts.set(versionKey + ":versionID", id)
        }
        let id = versionID!
        try checkAccount(); try await api.upload(versionID:id,artifact:bytes,bearer:token)
        try checkAccount()
        let submitted = try await api.publish("versions/\(id)/publish",body:[:],bearer:token,idempotencyKey:mutationKey("publish"))
        if case .object(let value) = submitted { publishStatus = value["status"]?.text ?? "Submitted" } else { publishStatus = "Submitted" }
    }
}
