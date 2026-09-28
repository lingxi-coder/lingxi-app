import Foundation
import XCTest

@testable import LingxiCode

final class RuntimeIntegrationTests: XCTestCase {
    func testDefaultWorkspaceRetainsValidPersistedIdentity() throws {
        let suite = "runtime-integration-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let id = "12345678-1234-4ABC-8DEF-1234567890AB"
        defaults.set(id, forKey: LXISHDefaultWorkspace.defaultsKey)
        XCTAssertEqual(LXISHDefaultWorkspace.stableID(defaults: defaults), id.lowercased())
    }

    func testInvalidWorkspaceIdentityIsReplacedOnce() throws {
        let suite = "runtime-integration-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("invalid", forKey: LXISHDefaultWorkspace.defaultsKey)
        let id = LXISHDefaultWorkspace.stableID(defaults: defaults)
        XCTAssertNotNil(UUID(uuidString: id))
        XCTAssertEqual(LXISHDefaultWorkspace.stableID(defaults: defaults), id)
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
