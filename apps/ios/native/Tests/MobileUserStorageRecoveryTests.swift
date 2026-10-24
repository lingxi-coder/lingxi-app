import Foundation
import XCTest
@testable import LingxiCode

final class MobileUserStorageRecoveryTests: XCTestCase {
    func testRelocatedContainerRecognitionIsLimitedToTheManagedWorkspace() {
        let home = "/var/mobile/Containers/Data/Application/11111111-1111-4111-8111-111111111111"
        let previous = "/private/var/mobile/Containers/Data/Application/22222222-2222-4222-8222-222222222222"
        let suffix = "/Library/Application Support/mobile-linux/ios-ish/persistent/lingxi/lingxi/projects"
        let projects = home + suffix
        XCTAssertTrue(MobileUserStorage.isRelocatedManagedWorkspaceLink(previous + suffix, projects: projects, applicationHome: home))
        for target in [previous + "/Documents/project", previous + suffix + "/source", home + suffix,
                       previous + "/../22222222-2222-4222-8222-222222222222" + suffix,
                       "/var/mobile/Containers/Data/Application/not-a-uuid" + suffix, "/tmp" + suffix] {
            XCTAssertFalse(MobileUserStorage.isRelocatedManagedWorkspaceLink(target, projects: projects, applicationHome: home), target)
        }
        XCTAssertFalse(MobileUserStorage.isRelocatedManagedWorkspaceLink(previous + suffix, projects: projects, applicationHome: previous))
        XCTAssertFalse(MobileUserStorage.isRelocatedManagedWorkspaceLink(previous + suffix, projects: home + "/Documents/projects", applicationHome: home))
    }

    func testSelfLinkIsPreservedBeforeProjectsDirectoryIsRecreated() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let paths = MobileUserStorage.directories(managedRoot: root.path)
        let projects = URL(fileURLWithPath: paths.projects)
        try FileManager.default.createDirectory(at: projects.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(atPath: projects.path, withDestinationPath: projects.path)
        let prepared = try MobileUserStorage.prepareProjectsDirectory(managedRoot: root.path)
        XCTAssertEqual(prepared.resolvingSymlinksInPath().path, projects.resolvingSymlinksInPath().path)
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: prepared.path)[.type] as? FileAttributeType, .typeDirectory)
        let recovery = URL(fileURLWithPath: paths.configHome).appendingPathComponent("recovery")
        let entries = try FileManager.default.contentsOfDirectory(at: recovery, includingPropertiesForKeys: nil)
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(try FileManager.default.destinationOfSymbolicLink(atPath: entries[0].appendingPathComponent("projects").path), projects.path)
        let receipt = try JSONSerialization.jsonObject(with: Data(contentsOf: entries[0].appendingPathComponent("receipt.json"))) as? [String: String]
        XCTAssertEqual(receipt?["linkTarget"], projects.path)
        _ = try MobileUserStorage.prepareProjectsDirectory(managedRoot: root.path)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(at: recovery, includingPropertiesForKeys: nil).count, 1)
    }

    func testExistingSourceAndForeignBrokenLinkAreNeverRemoved() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let projects = try MobileUserStorage.prepareProjectsDirectory(managedRoot: root.path)
        let source = projects.appendingPathComponent("source.txt")
        try Data("keep-source".utf8).write(to: source)
        _ = try MobileUserStorage.prepareProjectsDirectory(managedRoot: root.path)
        XCTAssertEqual(try Data(contentsOf: source), Data("keep-source".utf8))
        try FileManager.default.moveItem(at: projects, to: projects.deletingLastPathComponent().appendingPathComponent("preserved-source"))
        let missing = root.appendingPathComponent("missing-external-source").path
        try FileManager.default.createSymbolicLink(atPath: projects.path, withDestinationPath: missing)
        XCTAssertThrowsError(try MobileUserStorage.prepareProjectsDirectory(managedRoot: root.path))
        XCTAssertEqual(try FileManager.default.destinationOfSymbolicLink(atPath: projects.path), missing)
    }
}
