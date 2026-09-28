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
        unsetenv("LINGXI_IOS_RUNTIME_MANIFEST")
        unsetenv("LINGXI_IOS_AUTHORIZATION_MANIFEST")
        if temporaryRoot != nil { try? FileManager.default.removeItem(at: temporaryRoot) }
    }

    func testManifestLoaderReadsPinnedRootfsVersionAndArchiveHash() throws {
        let manifestURL = temporaryRoot.appendingPathComponent("linux-runtime-manifest.json")
        try """
        {
          "alpine_version": "3.24.1",
          "rootfs_zip_sha256": "abc123",
          "local_app_runtime": true
        }
        """.write(to: manifestURL, atomically: true, encoding: .utf8)
        setenv("LINGXI_IOS_RUNTIME_MANIFEST", manifestURL.path, 1)

        let manifest = LXISHRuntimeBundleMetadata.current()

        XCTAssertEqual(manifest.rootfsVersion, "3.24.1")
        XCTAssertEqual(manifest.archiveSha256, "abc123")
        XCTAssertTrue(manifest.localAppRuntime)
    }

    func testManifestLoaderFailsClosedWhenLocalAppRuntimeCapabilityIsMissing() throws {
        let manifestURL = temporaryRoot.appendingPathComponent("linux-runtime-manifest.json")
        try """
        {
          "alpine_version": "3.24.1",
          "rootfs_zip_sha256": "abc123"
        }
        """.write(to: manifestURL, atomically: true, encoding: .utf8)
        setenv("LINGXI_IOS_RUNTIME_MANIFEST", manifestURL.path, 1)

        XCTAssertFalse(LXISHRuntimeBundleMetadata.current().localAppRuntime)
    }

    func testLocalAppRuntimeRootRequiresRootfsCapabilityAndBuildTools() throws {
        let runtimeRoot = temporaryRoot.appendingPathComponent("local-app-runtime", isDirectory: true)
        let vite = runtimeRoot.appendingPathComponent("node_modules/vite/bin/vite.js")
        try FileManager.default.createDirectory(
            at: vite.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data().write(to: vite)

        XCTAssertNil(
            LocalAppsRuntimeDistribution.resolveRuntimeRoot(
                resourceURL: temporaryRoot,
                manifest: .init(
                    rootfsVersion: "3.24.1",
                    archiveSha256: "abc123",
                    localAppRuntime: false
                )
            )
        )
        XCTAssertEqual(
            LocalAppsRuntimeDistribution.resolveRuntimeRoot(
                resourceURL: temporaryRoot,
                manifest: .init(
                    rootfsVersion: "3.24.1",
                    archiveSha256: "abc123",
                    localAppRuntime: true
                )
            ),
            runtimeRoot.path
        )
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

    func testGuestPathAtlasMatchesRustTwin() {
        XCTAssertEqual(LXISHGuestPaths.home, "/root")
        XCTAssertEqual(LXISHGuestPaths.scratch, ["/tmp", "/var/tmp"])
        XCTAssertEqual(LXISHGuestPaths.workspaceRoot, "/workspace")
        XCTAssertEqual(LXISHGuestPaths.workspace("abc-123"), "/workspace/abc-123")
        XCTAssertEqual(LXISHGuestPaths.localAppBuildRoot, "/var/lingxi/local-app-build")
        XCTAssertEqual(LXISHGuestPaths.localAppBuildProjectDirectory, "project")
        XCTAssertEqual(
            LXISHGuestPaths.localAppBuildProject(appId: "abc-123", channel: "store"),
            "/var/lingxi/local-app-build/abc-123/store/project"
        )
    }
}
