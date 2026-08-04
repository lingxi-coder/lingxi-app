import CryptoKit
import Foundation
import XCTest

@testable import LingxiCode

final class LXISHRuntimeBundleManifestTests: XCTestCase {
    private var temporaryRoot: URL!

    override func setUpWithError() throws {
        temporaryRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("ios-ish-runtime-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: temporaryRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        unsetenv("LINGXI_IOS_RUNTIME_MANIFEST")
        unsetenv("LINGXI_IOS_AUTHORIZATION_MANIFEST")
        unsetenv("LINGXI_IOS_ISH_ROOTFS_ZIP")
        unsetenv("LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR")
        if temporaryRoot != nil {
            try? FileManager.default.removeItem(at: temporaryRoot)
        }
    }

    func testManifestLoaderReadsPinnedRootfsVersionAndArchiveHash() throws {
        let manifestURL = temporaryRoot.appendingPathComponent("linux-runtime-manifest.json")
        try """
        {
          "alpine_version": "3.24.1",
          "rootfs_zip_sha256": "abc123"
        }
        """.write(to: manifestURL, atomically: true, encoding: .utf8)
        setenv("LINGXI_IOS_RUNTIME_MANIFEST", manifestURL.path, 1)

        let manifest = LXISHRuntimeBundleMetadata.current()

        XCTAssertEqual(manifest.rootfsVersion, "3.24.1")
        XCTAssertEqual(manifest.archiveSha256, "abc123")
    }

    func testTerminalDescriptorFallsBackToBundledManifestWhenRuntimeStateIsEmpty() throws {
        let manifestURL = temporaryRoot.appendingPathComponent("linux-runtime-manifest.json")
        try """
        {
          "rootfs_version": "3.24.1",
          "archive_sha256": "feedbeef"
        }
        """.write(to: manifestURL, atomically: true, encoding: .utf8)
        setenv("LINGXI_IOS_RUNTIME_MANIFEST", manifestURL.path, 1)

        let project = ProjectSnapshot(
            record: ProjectRecord(
                id: "12345678-1234-4abc-8def-1234567890ab",
                name: "Project",
                storageKind: .internal,
                createdAt: .distantPast,
                updatedAt: .distantPast
            ),
            workspace: ProjectWorkspace(
                projectId: "12345678-1234-4abc-8def-1234567890ab",
                hostURL: URL(fileURLWithPath: "/tmp/project", isDirectory: true)
            ),
            sessions: []
        )
        var runtime = LinuxRuntimeState()
        runtime.selectedMode = .mobileLinux

        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: "/tmp/app",
            project: project,
            linuxRuntime: runtime
        )

        XCTAssertEqual(descriptor.config?.rootfsVersion, "3.24.1")
        XCTAssertEqual(descriptor.config?.archiveSha256, "feedbeef")
    }

    func testTerminalDescriptorUsesBundledManifestWhenInstalledRuntimeVersionIsStale() throws {
        let manifestURL = temporaryRoot.appendingPathComponent("linux-runtime-manifest.json")
        try """
        {
          "alpine_version": "3.24.1",
          "rootfs_zip_sha256": "feedbeef"
        }
        """.write(to: manifestURL, atomically: true, encoding: .utf8)
        setenv("LINGXI_IOS_RUNTIME_MANIFEST", manifestURL.path, 1)

        let project = ProjectSnapshot(
            record: ProjectRecord(
                id: "12345678-1234-4abc-8def-1234567890ab",
                name: "Project",
                storageKind: .internal,
                createdAt: .distantPast,
                updatedAt: .distantPast
            ),
            workspace: ProjectWorkspace(
                projectId: "12345678-1234-4abc-8def-1234567890ab",
                hostURL: URL(fileURLWithPath: "/tmp/project", isDirectory: true)
            ),
            sessions: []
        )
        var runtime = LinuxRuntimeState()
        runtime.selectedMode = .mobileLinux
        runtime.version = "3.21.0"

        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: "/tmp/app",
            project: project,
            linuxRuntime: runtime
        )

        XCTAssertEqual(descriptor.config?.rootfsVersion, "3.24.1")
        XCTAssertEqual(descriptor.config?.archiveSha256, "feedbeef")
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

    func testGuestEnvironmentPinsOnlyTheCurrentWorkspaceAsSafeDirectory() {
        let environment = LXISHGuestEnvironment.merged(
            requestEnvironment: [:],
            cwd: nil,
            stableWorkspaceId: "12345678-1234-4abc-8def-1234567890ab"
        )

        XCTAssertEqual(environment["HOME"], "/root")
        XCTAssertEqual(environment["PWD"], "/workspace/12345678-1234-4abc-8def-1234567890ab")
        XCTAssertEqual(environment["TMPDIR"], "/tmp")
        XCTAssertEqual(environment["PATH"], "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        XCTAssertEqual(environment["XDG_CACHE_HOME"], "/root/.cache")
        XCTAssertEqual(environment["NPM_CONFIG_CACHE"], "/root/.npm")
        XCTAssertEqual(environment["PIP_CACHE_DIR"], "/root/.cache/pip")
        XCTAssertEqual(environment["SSL_CERT_FILE"], "/etc/ssl/cert.pem")
        XCTAssertEqual(environment["GIT_CONFIG_COUNT"], "1")
        XCTAssertEqual(environment["GIT_CONFIG_KEY_0"], "safe.directory")
        XCTAssertEqual(environment["GIT_CONFIG_VALUE_0"], "/workspace/12345678-1234-4abc-8def-1234567890ab")
        XCTAssertNil(environment["GIT_CONFIG_KEY_1"])
    }

    func testGuestEnvironmentOverridesCallerProvidedSafeDirectoryEscape() {
        let environment = LXISHGuestEnvironment.merged(
            requestEnvironment: [
                "GIT_CONFIG_COUNT": "2",
                "GIT_CONFIG_KEY_0": "safe.directory",
                "GIT_CONFIG_VALUE_0": "*"
            ],
            cwd: nil,
            stableWorkspaceId: "12345678-1234-4abc-8def-1234567890ab"
        )

        XCTAssertEqual(environment["GIT_CONFIG_COUNT"], "1")
        XCTAssertEqual(environment["GIT_CONFIG_KEY_0"], "safe.directory")
        XCTAssertEqual(environment["GIT_CONFIG_VALUE_0"], "/workspace/12345678-1234-4abc-8def-1234567890ab")
    }

    func testRuntimeMountPlannerPreservesPersistentHomeAndWorkspaceLayers() {
        let config = LXISHNativeConfig(
            managedRoot: temporaryRoot.appendingPathComponent("managed-root", isDirectory: true).path,
            workspaceHostPath: temporaryRoot.appendingPathComponent("workspace-host", isDirectory: true).path,
            stableWorkspaceId: "12345678-1234-4abc-8def-1234567890ab",
            abi: "arm64",
            rootfsVersion: "3.24.1",
            archiveSha256: String(repeating: "a", count: 64),
            authorizationFile: nil
        )

        let mounts = LXISHRuntimeMountPlanner.effectiveMounts(
            requestedMounts: [
                LXISHMountSpec(
                    hostPath: temporaryRoot.appendingPathComponent("ignored-root", isDirectory: true).path,
                    guestPath: "/root",
                    readOnly: false,
                    purpose: "external"
                ),
                LXISHMountSpec(
                    hostPath: temporaryRoot.appendingPathComponent("workspace-subdir", isDirectory: true).path,
                    guestPath: "/workspace/12345678-1234-4abc-8def-1234567890ab/src",
                    readOnly: false,
                    purpose: "external"
                ),
            ],
            config: config
        )

        XCTAssertEqual(mounts.map(\.guestPath), [
            "/root",
            "/workspace/12345678-1234-4abc-8def-1234567890ab",
            "/workspace/12345678-1234-4abc-8def-1234567890ab/src",
        ])
        XCTAssertEqual(mounts[0].hostPath, config.persistentHomeURL.path)
        XCTAssertEqual(
            mounts[1].hostPath,
            URL(fileURLWithPath: config.workspaceHostPath, isDirectory: true).standardizedFileURL.path
        )
    }

    func testInstallVerifiesPinnedArchiveAndMigratesLegacyHomeIntoPersistentRoot() throws {
        let archive = try makeRootfsArchive(version: "3.24.1")
        let managedRoot = temporaryRoot.appendingPathComponent("managed-root", isDirectory: true)
        let defaultMount = temporaryRoot.appendingPathComponent("default_mount", isDirectory: true)
        try FileManager.default.createDirectory(at: defaultMount, withIntermediateDirectories: true)
        setenv("LINGXI_IOS_ISH_ROOTFS_ZIP", archive.url.path, 1)
        setenv("LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR", defaultMount.path, 1)

        let stableId = "12345678-1234-4abc-8def-1234567890ab"
        try seedExistingRootfs(at: managedRoot, stableWorkspaceId: stableId)

        let config = LXISHNativeConfig(
            managedRoot: managedRoot.path,
            workspaceHostPath: "/tmp/project",
            stableWorkspaceId: stableId,
            abi: "arm64",
            rootfsVersion: "3.24.1",
            archiveSha256: archive.sha256,
            authorizationFile: nil
        )
        let manager = LXISHNativeRootfsManager()

        let status = try manager.installIfNeeded(for: config)

        XCTAssertEqual(status.state, "ready")
        XCTAssertEqual(
            try String(contentsOf: config.rootfsURL.appendingPathComponent("etc/alpine-release"), encoding: .utf8),
            "3.24.1"
        )
        XCTAssertEqual(
            try String(contentsOf: config.persistentHomeURL.appendingPathComponent("notes.txt"), encoding: .utf8),
            "keep-me"
        )
        XCTAssertFalse(
            FileManager.default.fileExists(
                atPath: config.rootfsDataURL.appendingPathComponent("workspace/\(stableId)/hello.txt").path
            )
        )

        let drifted = LXISHNativeConfig(
            managedRoot: managedRoot.path,
            workspaceHostPath: "/tmp/project",
            stableWorkspaceId: stableId,
            abi: "arm64",
            rootfsVersion: "3.24.1",
            archiveSha256: String(repeating: "0", count: 64),
            authorizationFile: nil
        )
        XCTAssertEqual(manager.status(for: drifted).state, "corrupt")
    }

    func testInstallRejectsArchiveHashMismatch() throws {
        let archive = try makeRootfsArchive(version: "3.24.1")
        let managedRoot = temporaryRoot.appendingPathComponent("managed-root-mismatch", isDirectory: true)
        let defaultMount = temporaryRoot.appendingPathComponent("default_mount_mismatch", isDirectory: true)
        try FileManager.default.createDirectory(at: defaultMount, withIntermediateDirectories: true)
        setenv("LINGXI_IOS_ISH_ROOTFS_ZIP", archive.url.path, 1)
        setenv("LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR", defaultMount.path, 1)

        let config = LXISHNativeConfig(
            managedRoot: managedRoot.path,
            workspaceHostPath: "/tmp/project",
            stableWorkspaceId: "12345678-1234-4abc-8def-1234567890ab",
            abi: "arm64",
            rootfsVersion: "3.24.1",
            archiveSha256: String(repeating: "f", count: 64),
            authorizationFile: nil
        )

        XCTAssertThrowsError(try LXISHNativeRootfsManager().installIfNeeded(for: config))
    }

    func testInstallRejectsMissingArchiveHash() throws {
        let archive = try makeRootfsArchive(version: "3.24.1")
        let managedRoot = temporaryRoot.appendingPathComponent("managed-root-missing-hash", isDirectory: true)
        let defaultMount = temporaryRoot.appendingPathComponent("default_mount_missing_hash", isDirectory: true)
        try FileManager.default.createDirectory(at: defaultMount, withIntermediateDirectories: true)
        setenv("LINGXI_IOS_ISH_ROOTFS_ZIP", archive.url.path, 1)
        setenv("LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR", defaultMount.path, 1)

        let config = LXISHNativeConfig(
            managedRoot: managedRoot.path,
            workspaceHostPath: "/tmp/project",
            stableWorkspaceId: "12345678-1234-4abc-8def-1234567890ab",
            abi: "arm64",
            rootfsVersion: "3.24.1",
            archiveSha256: nil,
            authorizationFile: nil
        )

        XCTAssertThrowsError(try LXISHNativeRootfsManager().installIfNeeded(for: config))
    }

    private func seedExistingRootfs(at managedRoot: URL, stableWorkspaceId: String) throws {
        let rootfsURL = managedRoot.appendingPathComponent("alpine-rootfs", isDirectory: true)
        let dataURL = rootfsURL.appendingPathComponent("data", isDirectory: true)
        try FileManager.default.createDirectory(at: dataURL, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(
            at: rootfsURL.appendingPathComponent("etc", isDirectory: true),
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(at: dataURL.appendingPathComponent("root", isDirectory: true), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(
            at: dataURL.appendingPathComponent("workspace/\(stableWorkspaceId)", isDirectory: true),
            withIntermediateDirectories: true
        )
        try "old".write(to: rootfsURL.appendingPathComponent("meta.db"), atomically: true, encoding: .utf8)
        try "aarch64".write(to: rootfsURL.appendingPathComponent(".arch"), atomically: true, encoding: .utf8)
        try "3.21.0".write(to: rootfsURL.appendingPathComponent("etc/alpine-release"), atomically: true, encoding: .utf8)
        try "keep-me".write(to: dataURL.appendingPathComponent("root/notes.txt"), atomically: true, encoding: .utf8)
        try "workspace-data".write(
            to: dataURL.appendingPathComponent("workspace/\(stableWorkspaceId)/hello.txt"),
            atomically: true,
            encoding: .utf8
        )
        try """
        {
          "abi": "arm64",
          "rootfs_version": "3.21.0",
          "archive_sha256": "old",
          "arch": "aarch64",
          "updated_at": "2026-08-04T00:00:00Z"
        }
        """.write(to: rootfsURL.appendingPathComponent("bridge-state.json"), atomically: true, encoding: .utf8)
    }

    private func makeRootfsArchive(version: String) throws -> (url: URL, sha256: String) {
        let archiveURL = temporaryRoot.appendingPathComponent("alpine-rootfs-\(version).zip")
        let entries = [
            ("alpine-rootfs/meta.db", Data("meta".utf8)),
            ("alpine-rootfs/etc/alpine-release", Data(version.utf8)),
            ("alpine-rootfs/usr/bin/node", Data("#!/bin/sh\nexit 0\n".utf8)),
        ]
        let data = try TestZipArchive.make(entries: entries)
        try data.write(to: archiveURL, options: .atomic)
        let sha256 = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        return (archiveURL, sha256)
    }

    // MARK: - Guest mount points

    func testGuestMountPointMapsAbsolutePathsIntoTheFakefsDataTree() throws {
        let dataRoot = URL(fileURLWithPath: "/managed/alpine-rootfs/data", isDirectory: true)

        // A bind mount needs a directory on BOTH sides. Only the host side was
        // created, so every local-app build mount attached to nothing.
        let build = LXISHRuntimeMountPlanner.guestMountPointURL(
            for: "/var/lingxi/local-app-build/abcd1234/store",
            under: dataRoot
        )
        XCTAssertEqual(
            build?.standardizedFileURL.path,
            "/managed/alpine-rootfs/data/var/lingxi/local-app-build/abcd1234/store"
        )

        XCTAssertEqual(
            LXISHRuntimeMountPlanner.guestMountPointURL(for: "/root", under: dataRoot)?
                .standardizedFileURL.path,
            "/managed/alpine-rootfs/data/root"
        )
    }

    func testGuestMountPointRefusesPathsThatWouldEscapeTheDataTree() throws {
        let dataRoot = URL(fileURLWithPath: "/managed/alpine-rootfs/data", isDirectory: true)
        // Creating these would mkdir outside the rootfs on the HOST, so they are
        // refused rather than normalised.
        for guestPath in ["../escape", "/var/../../escape", "/a/./b", "relative/path", "/", ""] {
            XCTAssertNil(
                LXISHRuntimeMountPlanner.guestMountPointURL(for: guestPath, under: dataRoot),
                "guest path \(guestPath) must not map to a directory"
            )
        }
}

private enum TestZipArchive {
    struct Entry {
        var path: String
        var data: Data
    }

    static func make(entries: [(String, Data)]) throws -> Data {
        try make(entries: entries.map { Entry(path: $0.0, data: $0.1) })
    }

    static func make(entries: [Entry]) throws -> Data {
        var archive = Data()
        var centralDirectory = Data()
        var offset: UInt32 = 0

        for entry in entries {
            let name = Data(entry.path.utf8)
            let crc = CRC32.checksum(for: entry.data)
            archive.appendLE(UInt32(0x04034b50))
            archive.appendLE(UInt16(20))
            archive.appendLE(UInt16(0))
            archive.appendLE(UInt16(0))
            archive.appendLE(UInt16(0))
            archive.appendLE(UInt16(0))
            archive.appendLE(crc)
            archive.appendLE(UInt32(entry.data.count))
            archive.appendLE(UInt32(entry.data.count))
            archive.appendLE(UInt16(name.count))
            archive.appendLE(UInt16(0))
            archive.append(name)
            archive.append(entry.data)

            centralDirectory.appendLE(UInt32(0x02014b50))
            centralDirectory.appendLE(UInt16(20))
            centralDirectory.appendLE(UInt16(20))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(crc)
            centralDirectory.appendLE(UInt32(entry.data.count))
            centralDirectory.appendLE(UInt32(entry.data.count))
            centralDirectory.appendLE(UInt16(name.count))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt16(0))
            centralDirectory.appendLE(UInt32(0))
            centralDirectory.appendLE(offset)
            centralDirectory.append(name)

            offset = UInt32(archive.count)
        }

        let centralDirectoryOffset = UInt32(archive.count)
        archive.append(centralDirectory)
        archive.appendLE(UInt32(0x06054b50))
        archive.appendLE(UInt16(0))
        archive.appendLE(UInt16(0))
        archive.appendLE(UInt16(entries.count))
        archive.appendLE(UInt16(entries.count))
        archive.appendLE(UInt32(centralDirectory.count))
        archive.appendLE(centralDirectoryOffset)
        archive.appendLE(UInt16(0))
        return archive
    }
}

private enum CRC32 {
    static func checksum(for data: Data) -> UInt32 {
        var crc: UInt32 = 0xffff_ffff
        for byte in data {
            crc ^= UInt32(byte)
            for _ in 0..<8 {
                let mask = (crc & 1) == 1 ? UInt32(0xedb8_8320) : 0
                crc = (crc >> 1) ^ mask
            }
        }
        return crc ^ 0xffff_ffff
    }
    }
}

private extension Data {
    mutating func appendLE(_ value: UInt16) {
        var value = value.littleEndian
        append(Data(bytes: &value, count: MemoryLayout<UInt16>.size))
    }

    mutating func appendLE(_ value: UInt32) {
        var value = value.littleEndian
        append(Data(bytes: &value, count: MemoryLayout<UInt32>.size))
    }

}
