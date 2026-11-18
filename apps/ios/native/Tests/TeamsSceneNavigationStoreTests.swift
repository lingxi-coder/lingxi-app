import XCTest
@testable import LingxiCode

final class TeamsSceneNavigationStoreTests: XCTestCase {
    func testLatestConversationSurvivesAStoreReopenAndScenesStaySeparate() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = TeamsSceneNavigationStore(directory: directory)
        try store.write("alice\nteam-1\nolder-chat", sceneID: "scene-1")
        try store.write("alice\nteam-1\nlatest-chat", sceneID: "scene-1")
        try store.write("bob\nteam-2\nother-chat", sceneID: "scene-2")

        let reopened = TeamsSceneNavigationStore(directory: directory)
        XCTAssertEqual(try reopened.read(sceneID: "scene-1"), "alice\nteam-1\nlatest-chat")
        XCTAssertEqual(try reopened.read(sceneID: "scene-2"), "bob\nteam-2\nother-chat")
        XCTAssertNil(try reopened.read(sceneID: "unknown-scene"))
    }

    func testReturningToTheListPersistsAnExplicitEmptySnapshot() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = TeamsSceneNavigationStore(directory: directory)
        try store.write("alice\nteam-1\nchat", sceneID: "scene")
        try store.write("", sceneID: "scene")
        XCTAssertEqual(try TeamsSceneNavigationStore(directory: directory).read(sceneID: "scene"), "",
                       "An empty persisted value must suppress an older SceneStorage conversation")
    }

    func testSceneIdentifiersCannotEscapeThePrivateDirectory() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = TeamsSceneNavigationStore(directory: directory)
        try store.write("scoped-value", sceneID: "../../another/scene")
        XCTAssertEqual(try store.read(sceneID: "../../another/scene"), "scoped-value")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: directory.path).count, 1)
        XCTAssertThrowsError(try store.write("value", sceneID: ""))
        XCTAssertThrowsError(try store.write("value", sceneID: String(repeating: "x", count: 161)))
    }
}
