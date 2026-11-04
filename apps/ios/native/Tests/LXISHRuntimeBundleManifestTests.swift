import Darwin
import Foundation
import XCTest

@testable import LingxiCode

/// Product resource/default tests. Native lifecycle regressions live with the SDK.
final class LXISHRuntimeBundleManifestTests: XCTestCase {
    private var temporaryRoot: URL!

    override func setUpWithError() throws {
        temporaryRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("ios-runtime-host-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: temporaryRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        unsetenv("LINGXI_IOS_AUTHORIZATION_MANIFEST")
        if temporaryRoot != nil { try? FileManager.default.removeItem(at: temporaryRoot) }
    }

    func testManifestLoaderReadsPinnedRootfsVersionAndArchiveHash() throws {
        let directories = MobileUserStorage.directories()
        let archive = try writeArchiveAndManifest(profile: ["brand_id": directories.brandId,
            "linux_user": directories.linuxUser, "config_dir_name": directories.configDirName])
        let manifest = try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)
        XCTAssertEqual(manifest.rootfsVersion, "3.24.1")
        XCTAssertEqual(manifest.archiveSha256, String(repeating: "a", count: 64))
    }

    func testMissingOrMismatchedBrandProfileCannotAuthorizeRuntime() throws {
        let directories = MobileUserStorage.directories()
        let correct = ["brand_id": directories.brandId, "linux_user": directories.linuxUser,
                       "config_dir_name": directories.configDirName]
        for key in correct.keys {
            var mismatch = correct
            mismatch[key] = "wrong"
            let archive = try writeArchiveAndManifest(profile: mismatch)
            XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
                XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .profileMismatch)
            }
            var missing = correct
            missing.removeValue(forKey: key)
            _ = try writeArchiveAndManifest(profile: missing)
            XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
                XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .invalid)
            }
        }
        let archive = try writeArchiveAndManifest(profile: nil)
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .invalid)
        }
        var extra = correct
        extra["other"] = "not part of the profile"
        _ = try writeArchiveAndManifest(profile: extra)
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .invalid)
        }
    }

    func testArchiveRequiresCurrentSidecarAndValidDigest() throws {
        let directories = MobileUserStorage.directories()
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: nil, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .missing)
        }
        let archive = temporaryRoot.appendingPathComponent("alpine-rootfs.zip")
        try Data().write(to: archive)
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .missing)
        }
        let profile = ["brand_id": directories.brandId, "linux_user": directories.linuxUser,
                       "config_dir_name": directories.configDirName]
        _ = try writeArchiveAndManifest(profile: profile, digest: "old-format")
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .invalid)
        }
        let sidecar = temporaryRoot.appendingPathComponent("linux-runtime-manifest.json")
        try FileManager.default.removeItem(at: sidecar)
        XCTAssertEqual(mkfifo(sidecar.path, 0o600), 0)
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .invalid)
        }
        try FileManager.default.removeItem(at: sidecar)
        try Data(repeating: 32, count: 65_537).write(to: sidecar)
        XCTAssertThrowsError(try LXISHRuntimeBundleMetadata.load(archiveURL: archive, directories: directories)) {
            XCTAssertEqual($0 as? LXISHRuntimeBundleManifestError, .invalid)
        }
    }

    @MainActor
    func testMismatchedBundleDoesNotCreateTerminalRuntimeConfiguration() throws {
        let directories = MobileUserStorage.directories()
        let archive = try writeArchiveAndManifest(profile: ["brand_id": "wrong", "linux_user": directories.linuxUser,
            "config_dir_name": directories.configDirName])
        let key = LXISHRuntimeResourceBootstrap.archiveEnvironmentKey
        let previous = ProcessInfo.processInfo.environment[key]
        setenv(key, archive.path, 1)
        defer {
            if let previous { setenv(key, previous, 1) }
            else { unsetenv(key) }
        }
        let descriptor = TerminalRuntimeDescriptor.make(appSandboxRoot: temporaryRoot.path, project: nil,
                                                        linuxRuntime: LinuxRuntimeState())
        XCTAssertNil(descriptor.config, "a different brand's bundle must not configure an installable runtime")
    }

    private func writeArchiveAndManifest(profile: [String: String]?, digest: String = String(repeating: "a", count: 64)) throws -> URL {
        let archive = temporaryRoot.appendingPathComponent("alpine-rootfs.zip")
        try Data().write(to: archive)
        var manifest: [String: Any] = ["schema_version": 1, "platform": "ios", "guest_abi": "aarch64",
            "rootfs_version": "3.24.1", "rootfs_zip_sha256": digest]
        manifest["brand_profile"] = profile
        try JSONSerialization.data(withJSONObject: manifest).write(to: temporaryRoot.appendingPathComponent("linux-runtime-manifest.json"))
        return archive
    }

    func testAuthorizationManifestURLPrefersExplicitEnvironmentOverride() throws {
        let authorizationURL = temporaryRoot.appendingPathComponent("AUTHORIZATION_MANIFEST.json")
        try "{}".write(to: authorizationURL, atomically: true, encoding: .utf8)
        setenv("LINGXI_IOS_AUTHORIZATION_MANIFEST", authorizationURL.path, 1)

        XCTAssertEqual(
            LXISHRuntimeBundleResources.authorizationManifestURL()?.standardizedFileURL,
            authorizationURL.standardizedFileURL
        )
    }

    func testGuestPathAtlasMatchesRustTwin() {
        let directories = MobileUserStorage.directories()
        XCTAssertEqual(LXISHGuestPaths.home, directories.guestHome)
        XCTAssertEqual(LXISHGuestPaths.scratch, ["/tmp", "/var/tmp"])
        XCTAssertEqual(LXISHGuestPaths.workspaceRoot, directories.configGuestPath)
        XCTAssertEqual(LXISHGuestPaths.conversationWorkspace(for: .chat), directories.chatGuestCwd)
        XCTAssertEqual(LXISHGuestPaths.conversationWorkspace(for: .code), directories.codeGuestCwd)
        XCTAssertNotEqual(directories.chatGuestCwd, directories.codeGuestCwd)
        XCTAssertEqual(LXISHGuestPaths.workspace("projects"), directories.projectsGuestPath)
        XCTAssertEqual(LXISHGuestPaths.workspace("scheduled"), directories.scheduledGuestCwd)
        XCTAssertEqual(LXISHGuestPaths.workspace("abc-123"), directories.configGuestPath + "/abc-123")
    }
}
