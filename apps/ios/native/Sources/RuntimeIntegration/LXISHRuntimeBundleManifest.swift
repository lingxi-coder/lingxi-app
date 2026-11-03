import Darwin
import Foundation

struct LXISHRuntimeBundleManifest: Equatable {
    var rootfsVersion: String
    var archiveSha256: String
}

enum LXISHRuntimeBundleManifestError: LocalizedError, Equatable {
    case missing
    case invalid
    case profileMismatch

    var errorDescription: String? {
        switch self {
        case .missing: String(localized: "settings_linux_not_installed")
        case .invalid, .profileMismatch: String(localized: "settings_linux_state_corrupt")
        }
    }
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

    private static func firstExistingURL(in candidates: [URL?], fileManager: FileManager) -> URL? {
        candidates
            .compactMap { $0 }
            .first(where: { fileManager.fileExists(atPath: $0.path) })
    }
}

enum LXISHRuntimeBundleMetadata {
    static func current(
        bundle: Bundle = .main,
        processInfo: ProcessInfo = .processInfo,
        fileManager: FileManager = .default
    ) throws -> LXISHRuntimeBundleManifest {
        let archive = LXISHRuntimeResourceBootstrap.rootfsArchiveURL(
            managedRoot: LXISHDefaultWorkspace.managedRootPath(), bundle: bundle,
            environment: processInfo.environment, fileManager: fileManager)
        return try load(archiveURL: archive, directories: MobileUserStorage.directories(), fileManager: fileManager)
    }

    static func load(archiveURL: URL?, directories: MobileUserDirectories,
                     fileManager: FileManager = .default) throws -> LXISHRuntimeBundleManifest {
        guard let archiveURL, fileManager.fileExists(atPath: archiveURL.path) else {
            throw LXISHRuntimeBundleManifestError.missing
        }
        var archiveInfo = stat()
        guard lstat(archiveURL.path, &archiveInfo) == 0, (archiveInfo.st_mode & S_IFMT) == S_IFREG else {
            throw LXISHRuntimeBundleManifestError.invalid
        }
        let url = archiveURL.deletingLastPathComponent().appendingPathComponent("linux-runtime-manifest.json")
        guard fileManager.fileExists(atPath: url.path) else { throw LXISHRuntimeBundleManifestError.missing }
        return try parseManifest(at: url, directories: directories)
    }

    private struct BrandProfile: Decodable, Equatable {
        let brandID: String
        let linuxUser: String
        let configDirName: String

        enum CodingKeys: String, CodingKey {
            case brandID = "brand_id", linuxUser = "linux_user", configDirName = "config_dir_name"
        }

        private struct AnyKey: CodingKey {
            let stringValue: String
            let intValue: Int? = nil
            init?(stringValue: String) { self.stringValue = stringValue }
            init?(intValue: Int) { return nil }
        }

        init(brandID: String, linuxUser: String, configDirName: String) {
            self.brandID = brandID
            self.linuxUser = linuxUser
            self.configDirName = configDirName
        }

        init(from decoder: Decoder) throws {
            let all = try decoder.container(keyedBy: AnyKey.self)
            guard Set(all.allKeys.map(\.stringValue)) == Set(["brand_id", "linux_user", "config_dir_name"]) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "Invalid brand profile keys"))
            }
            let values = try decoder.container(keyedBy: CodingKeys.self)
            brandID = try values.decode(String.self, forKey: .brandID)
            linuxUser = try values.decode(String.self, forKey: .linuxUser)
            configDirName = try values.decode(String.self, forKey: .configDirName)
        }
    }

    private struct Manifest: Decodable {
        let schemaVersion: Int
        let platform: String
        let guestABI: String
        let rootfsVersion: String
        let archiveSha256: String
        let brandProfile: BrandProfile

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version", platform, guestABI = "guest_abi"
            case rootfsVersion = "rootfs_version", archiveSha256 = "rootfs_zip_sha256", brandProfile = "brand_profile"
        }
    }

    private static func parseManifest(at url: URL, directories: MobileUserDirectories) throws -> LXISHRuntimeBundleManifest {
        let descriptor = open(url.path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK)
        guard descriptor >= 0 else { throw LXISHRuntimeBundleManifestError.invalid }
        defer { _ = Darwin.close(descriptor) }
        var metadata = stat()
        guard fstat(descriptor, &metadata) == 0, (metadata.st_mode & S_IFMT) == S_IFREG,
              metadata.st_size > 0, metadata.st_size <= 65_536,
              let data = try? FileHandle(fileDescriptor: descriptor, closeOnDealloc: false).read(upToCount: 65_537),
              data.count <= 65_536,
              let manifest = try? JSONDecoder().decode(Manifest.self, from: data),
              manifest.schemaVersion == 1, manifest.platform == "ios", manifest.guestABI == "aarch64",
              !manifest.rootfsVersion.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              manifest.archiveSha256.count == 64,
              manifest.archiveSha256.allSatisfy({ $0.isASCII && ($0.isNumber || ("a"..."f").contains($0)) })
        else { throw LXISHRuntimeBundleManifestError.invalid }
        let expected = BrandProfile(brandID: directories.brandId, linuxUser: directories.linuxUser,
                                    configDirName: directories.configDirName)
        guard manifest.brandProfile == expected else { throw LXISHRuntimeBundleManifestError.profileMismatch }
        return LXISHRuntimeBundleManifest(rootfsVersion: manifest.rootfsVersion, archiveSha256: manifest.archiveSha256)
    }
}
