import Foundation

struct LXISHRuntimeBundleManifest: Equatable {
    var rootfsVersion: String
    var archiveSha256: String?

    static let fallback = LXISHRuntimeBundleManifest(rootfsVersion: "1.0.0", archiveSha256: nil)
}

enum LXISHRuntimeBundleResources {
    private static let envAuthorizationManifestPath = "LINGXI_IOS_AUTHORIZATION_MANIFEST"

    static func authorizationManifestURL(
        bundle: Bundle = .main,
        processInfo: ProcessInfo = .processInfo,
        fileManager: FileManager = .default
    ) -> URL? {
        let env = processInfo.environment
        let candidates: [URL?] = [
            env[envAuthorizationManifestPath].map { URL(fileURLWithPath: $0) },
            bundle.url(forResource: "AUTHORIZATION_MANIFEST", withExtension: "json"),
            bundle.resourceURL?.appendingPathComponent("AUTHORIZATION_MANIFEST.json"),
        ]
        return firstExistingURL(in: candidates, fileManager: fileManager)
    }

    static func manifestURLs(
        bundle: Bundle = .main,
        processInfo: ProcessInfo = .processInfo
    ) -> [URL] {
        let env = processInfo.environment
        let candidates: [URL?] = [
            env[LXISHRuntimeBundleMetadata.envManifestPath].map { URL(fileURLWithPath: $0) },
            bundle.url(forResource: "linux-runtime-manifest", withExtension: "json"),
            bundle.url(forResource: "runtime-manifest", withExtension: "json"),
            bundle.url(forResource: "manifest", withExtension: "json"),
            bundle.resourceURL?.appendingPathComponent("linux-runtime-manifest.json"),
            bundle.resourceURL?.appendingPathComponent("runtime-manifest.json"),
            bundle.resourceURL?.appendingPathComponent("manifest.json"),
        ]
        return candidates.compactMap { $0 }
    }

    private static func firstExistingURL(in candidates: [URL?], fileManager: FileManager) -> URL? {
        candidates
            .compactMap { $0 }
            .first(where: { fileManager.fileExists(atPath: $0.path) })
    }
}

enum LXISHRuntimeBundleMetadata {
    static let envManifestPath = "LINGXI_IOS_RUNTIME_MANIFEST"

    static func current(
        bundle: Bundle = .main,
        processInfo: ProcessInfo = .processInfo,
        fileManager: FileManager = .default
    ) -> LXISHRuntimeBundleManifest {
        for url in LXISHRuntimeBundleResources.manifestURLs(bundle: bundle, processInfo: processInfo) {
            guard fileManager.fileExists(atPath: url.path),
                  let manifest = parseManifest(at: url)
            else {
                continue
            }
            return manifest
        }
        return .fallback
    }

    private static func parseManifest(at url: URL) -> LXISHRuntimeBundleManifest? {
        guard let data = try? Data(contentsOf: url),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return nil
        }
        let version = string(in: json, keys: ["rootfs_version", "rootfsVersion", "alpine_version", "alpineVersion"])
            ?? LXISHRuntimeBundleManifest.fallback.rootfsVersion
        let archiveSha256 = string(
            in: json,
            keys: ["archive_sha256", "archiveSha256", "rootfs_zip_sha256", "rootfsZipSha256"]
        )
        return LXISHRuntimeBundleManifest(rootfsVersion: version, archiveSha256: archiveSha256)
    }

    private static func string(in json: [String: Any], keys: [String]) -> String? {
        for key in keys {
            if let value = json[key] as? String,
               !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            {
                return value.trimmingCharacters(in: .whitespacesAndNewlines)
            }
        }
        return nil
    }
}
