import Foundation
import XCTest

@testable import LingxiCode

final class RuntimeIntegrationTests: XCTestCase {
    func testWorkspaceMountIdentityIsIndependentOfProjectsAndPreferences() {
        XCTAssertEqual(LXISHDefaultWorkspace.stableID(), "projects")
        XCTAssertEqual(LXISHDefaultWorkspace.stableID(), LXISHDefaultWorkspace.stableID())
        XCTAssertEqual(LXISHDefaultWorkspace.guestHome, LXISHGuestPaths.workspaceRoot)
    }

    func testUserStorageNormalizesContainerAliasesBeforeDerivingProjects() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let actual = root.appendingPathComponent("actual", isDirectory: true)
        let alias = root.appendingPathComponent("alias", isDirectory: true)
        try FileManager.default.createDirectory(at: actual, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: actual)
        let paths = MobileUserStorage.directories(managedRoot: alias.appendingPathComponent("runtime").path)
        XCTAssertTrue(paths.projects.hasPrefix(actual.resolvingSymlinksInPath().path + "/"))
        let repository = ProjectRepository(projectsRoot: URL(fileURLWithPath: paths.projects), userHome: URL(fileURLWithPath: paths.home))
        let created = try repository.createInternal(name: "Container alias")
        XCTAssertEqual(repository.load().projects.map(\.id), [created.id])
    }

    func testProjectMountGrantUsesExactProjectSourceAndRejectsBroadPaths() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let managedRoot = root.appendingPathComponent("runtime", isDirectory: true).path
        let paths = MobileUserStorage.directories(managedRoot: managedRoot)
        let sourceGuest = paths.appsGuestPath + "/calorie-a1b2c3d4"
        let sourceHost = URL(fileURLWithPath: paths.apps, isDirectory: true)
            .appendingPathComponent("calorie-a1b2c3d4", isDirectory: true)
        try FileManager.default.createDirectory(at: sourceHost, withIntermediateDirectories: true)

        XCTAssertEqual(
            try MobileUserStorage.projectMountGrantPaths(
                activeDirectory: sourceGuest,
                additionalDirectories: [sourceGuest],
                managedRoot: managedRoot
            ),
            [sourceGuest]
        )

        for broadGuest in [
            paths.guestHome,
            paths.configGuestPath,
            paths.appsGuestPath,
            "/tmp/unapproved-project"
        ] {
            XCTAssertThrowsError(
                try MobileUserStorage.projectMountGrantPaths(
                    activeDirectory: broadGuest,
                    additionalDirectories: [],
                    managedRoot: managedRoot
                ),
                "broad path must not become a project bind mount: \(broadGuest)"
            )
        }

        let aliasGuest = paths.appsGuestPath + "/alias"
        let aliasHost = URL(fileURLWithPath: paths.apps, isDirectory: true)
            .appendingPathComponent("alias", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: aliasHost, withDestinationURL: sourceHost)
        XCTAssertThrowsError(
            try MobileUserStorage.projectMountGrantPaths(
                activeDirectory: aliasGuest,
                additionalDirectories: [],
                managedRoot: managedRoot
            ),
            "symlink source must not become a bind mount"
        )
    }

    func testProjectMountGrantsRejectDuplicateAndOverlappingDirectoryRecords() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let managedRoot = root.appendingPathComponent("runtime", isDirectory: true).path
        let paths = MobileUserStorage.directories(managedRoot: managedRoot)
        let parentGuest = paths.appsGuestPath + "/team"
        let childGuest = parentGuest + "/nested"
        let appsHost = URL(fileURLWithPath: paths.apps, isDirectory: true)
        try FileManager.default.createDirectory(
            at: appsHost.appendingPathComponent("team/nested", isDirectory: true),
            withIntermediateDirectories: true
        )

        XCTAssertThrowsError(
            try MobileUserStorage.projectMountGrantPaths(
                activeDirectory: parentGuest,
                additionalDirectories: [parentGuest, childGuest],
                managedRoot: managedRoot
            )
        )
    }

    func testExplicitResourceOverridesWinOverManagedResources() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let managed = root.appendingPathComponent("managed")
        let archive = root.appendingPathComponent("explicit.zip")
        let mount = root.appendingPathComponent("explicit-mount", isDirectory: true)
        try FileManager.default.createDirectory(at: managed.appendingPathComponent("resources/default_mount"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: mount, withIntermediateDirectories: true)
        try Data().write(to: archive)
        try Data().write(to: managed.appendingPathComponent("resources/alpine-rootfs.zip"))
        let environment = [
            LXISHRuntimeResourceBootstrap.archiveEnvironmentKey: archive.path,
            LXISHRuntimeResourceBootstrap.defaultMountEnvironmentKey: mount.path,
        ]
        XCTAssertEqual(LXISHRuntimeResourceBootstrap.rootfsArchiveURL(managedRoot: managed.path, environment: environment), archive)
        XCTAssertEqual(LXISHRuntimeResourceBootstrap.defaultMountURL(managedRoot: managed.path, environment: environment), mount)
    }

    func testInvalidAndEmptyResourceOverridesRetainManagedFallback() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let bundle = try emptyBundle(in: root)
        let resources = root.appendingPathComponent("managed/resources")
        try FileManager.default.createDirectory(at: resources.appendingPathComponent("default_mount"), withIntermediateDirectories: true)
        try Data().write(to: resources.appendingPathComponent("alpine-rootfs.zip"))
        for override in ["", root.appendingPathComponent("missing").path] {
            let resolved = LXISHRuntimeResourceBootstrap.resolvedLegacyResourceOverrides(
                managedRoot: root.appendingPathComponent("managed").path,
                bundle: bundle,
                environment: [
                    LXISHRuntimeResourceBootstrap.archiveEnvironmentKey: override,
                    LXISHRuntimeResourceBootstrap.defaultMountEnvironmentKey: override,
                ]
            )
            XCTAssertEqual(resolved[LXISHRuntimeResourceBootstrap.archiveEnvironmentKey], resources.appendingPathComponent("alpine-rootfs.zip").path)
            XCTAssertEqual(resolved[LXISHRuntimeResourceBootstrap.defaultMountEnvironmentKey], resources.appendingPathComponent("default_mount").path)
        }
    }

    func testInvalidResourceOverridesUseBundleBeforeManagedResources() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let bundle = try emptyBundle(in: root)
        let resources = try XCTUnwrap(bundle.resourceURL)
        try Data().write(to: resources.appendingPathComponent("alpine-rootfs.zip"))
        try FileManager.default.createDirectory(at: resources.appendingPathComponent("default_mount"), withIntermediateDirectories: true)
        let managed = root.appendingPathComponent("managed/resources")
        try FileManager.default.createDirectory(at: managed.appendingPathComponent("default_mount"), withIntermediateDirectories: true)
        try Data().write(to: managed.appendingPathComponent("alpine-rootfs.zip"))
        let resolved = LXISHRuntimeResourceBootstrap.resolvedLegacyResourceOverrides(
            managedRoot: root.appendingPathComponent("managed").path,
            bundle: bundle,
            environment: [
                LXISHRuntimeResourceBootstrap.archiveEnvironmentKey: root.appendingPathComponent("missing.zip").path,
                LXISHRuntimeResourceBootstrap.defaultMountEnvironmentKey: "",
            ]
        )
        XCTAssertEqual(resolved[LXISHRuntimeResourceBootstrap.archiveEnvironmentKey], resources.appendingPathComponent("alpine-rootfs.zip").path)
        XCTAssertEqual(resolved[LXISHRuntimeResourceBootstrap.defaultMountEnvironmentKey], resources.appendingPathComponent("default_mount").path)
    }

    func testInvalidResourceOverridesWithoutFallbackAreNotForwarded() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let bundle = try emptyBundle(in: root)
        for override in ["", root.appendingPathComponent("missing").path] {
            let resolved = LXISHRuntimeResourceBootstrap.resolvedLegacyResourceOverrides(
                managedRoot: "",
                bundle: bundle,
                environment: [
                    LXISHRuntimeResourceBootstrap.archiveEnvironmentKey: override,
                    LXISHRuntimeResourceBootstrap.defaultMountEnvironmentKey: override,
                ]
            )
            XCTAssertTrue(resolved.isEmpty)
        }
    }

    private func emptyBundle(in root: URL) throws -> Bundle {
        let url = root.appendingPathComponent("Empty.bundle")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        let info = try PropertyListSerialization.data(fromPropertyList: ["CFBundleIdentifier": "runtime-integration.empty"], format: .xml, options: 0)
        try info.write(to: url.appendingPathComponent("Info.plist"))
        return try XCTUnwrap(Bundle(url: url))
    }

    func testManagedResourceFallbackDoesNotInstallOrMutateRootfs() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let resources = root.appendingPathComponent("resources")
        try FileManager.default.createDirectory(at: resources.appendingPathComponent("default_mount"), withIntermediateDirectories: true)
        try Data().write(to: resources.appendingPathComponent("alpine-rootfs.zip"))
        XCTAssertEqual(LXISHRuntimeResourceBootstrap.rootfsArchiveURL(managedRoot: root.path, environment: [:])?.lastPathComponent, "alpine-rootfs.zip")
        XCTAssertEqual(LXISHRuntimeResourceBootstrap.defaultMountURL(managedRoot: root.path, environment: [:])?.lastPathComponent, "default_mount")
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("alpine-rootfs").path))
    }
}
