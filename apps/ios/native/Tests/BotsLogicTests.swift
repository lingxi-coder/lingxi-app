import XCTest
@testable import LingxiCode

final class BotsLogicTests: XCTestCase {
    func testChatWindowsMatchSharedVectors() throws {
        for row in try MobileVectors.rows("mobile", "chatWindow") {
            let actual = BotsLogic.chatWindow(total: row["total"] as! Int, expanded: row["expanded"] as! Bool)
            let expected = row["expect"] as! [String: Any]
            XCTAssertEqual(actual.shown, expected["shown"] as? Int)
            XCTAssertEqual(actual.hidden, expected["hidden"] as? Int)
            XCTAssertEqual(actual.showMore, expected["showMore"] as? Bool)
            XCTAssertEqual(actual.showLess, expected["showLess"] as? Bool)
        }
    }

    func testBotAndHostSortingMatchSharedVectors() throws {
        let bots = try MobileVectors.section("mobile", "sortBots") as! [String: Any]
        let items = (bots["input"] as! [[String: Any]]).map {
            BotsItem(id: $0["botId"] as! String, name: $0["name"] as! String,
                     lastActivityAtMs: ($0["lastActivityAtMs"] as? NSNumber)?.uint64Value)
        }
        XCTAssertEqual(BotsLogic.sortBots(items).map(\.id), bots["expect"] as? [String])
        let hosts = try MobileVectors.section("mobile", "sortHosts") as! [String: Any]
        let sorted = BotsLogic.sortHosts((hosts["input"] as! [[String: Any]]).map(host), connectedHostID: hosts["connected"] as? String)
        XCTAssertEqual(sorted.map(\.id), hosts["expect"] as? [String])
    }

    func testEveryHostBodyMatchesTheSharedStateTable() throws {
        let rows = try MobileVectors.rows("mobile", "hostBody")
        XCTAssertEqual(rows.count, 13)
        for row in rows {
            let input = row["input"] as! [String: Any], expected = row["expect"] as! [String: Any]
            let live: BotsLogic.Live
            if let count = input["live"] as? Int { live = .bots(count) }
            else { live = input["live"] as? String == "unsupported" ? .unsupported : .unknown }
            let actual = BotsLogic.hostBody(connected: input["connected"] as! Bool, online: input["online"] as! Bool,
                paired: input["paired"] as! Bool, live: live, snapshotBots: input["snapshot"] as? Int)
            XCTAssertEqual(actual.kind.rawValue, expected["kind"] as? String, row["name"] as! String)
            XCTAssertEqual(actual.source.rawValue, expected["source"] as? String)
            XCTAssertEqual(actual.dimmed, expected["dimmed"] as? Bool)
            XCTAssertEqual(actual.actionable, expected["actionable"] as? Bool)
            XCTAssertEqual(actual.notes.map(\.rawValue), expected["notes"] as? [String])
        }
    }

    func testAutoConnectOnlyUsesLastPairedOnlineComputer() throws {
        for row in try MobileVectors.rows("mobile", "autoConnectTarget") {
            let input = row["input"] as! [String: Any]
            let target = BotsLogic.autoConnectTarget(signedIn: input["signedIn"] as! Bool,
                connectedHostID: input["connectedHostId"] as? String, lastHostID: input["lastHostId"] as? String,
                hosts: (input["hosts"] as! [[String: Any]]).map(host))
            XCTAssertEqual(target, row["expect"] as? String, row["name"] as! String)
        }
    }

    func testSnapshotUpdatesMatchSharedVectorsAndKeepHostChatOrder() throws {
        for row in try MobileVectors.rows("mobile", "snapshot", "applyBots") {
            let actual = BotsLogic.applyBots(prior: snapshot(row["prior"]), uid: "u", hostID: row["hostId"] as! String,
                bots: (row["live"] as! [String]).map { BotsItem(id: $0) }, nowMs: (row["now"] as! NSNumber).uint64Value)
            if let expected = snapshot(row["expect"]) { assertShape(actual, expected) }
            else {
                XCTAssertEqual(actual.bots.count, row["expectCount"] as? Int)
                XCTAssertEqual(actual.bots.first?.id, "b1"); XCTAssertEqual(actual.bots.last?.id, "b100")
                XCTAssertTrue(actual.bots.allSatisfy { $0.chats == nil })
            }
        }
        for row in try MobileVectors.rows("mobile", "snapshot", "applyChats") {
            let actual = BotsLogic.applyChats(prior: snapshot(row["prior"]), botID: row["botId"] as! String,
                chats: (row["chats"] as! [String]).map { BotsChat(id: $0) }, nowMs: (row["now"] as! NSNumber).uint64Value)
            if let expected = snapshot(row["expect"]) { assertShape(try XCTUnwrap(actual), expected) }
            else if let count = row["expectCount"] as? Int {
                let chats = try XCTUnwrap(actual?.bots.first { $0.id == row["botId"] as? String }?.chats)
                XCTAssertEqual(chats.count, count)
                XCTAssertEqual([chats.first!.id, chats.last!.id], row["expectFirstLast"] as? [String])
            } else { XCTAssertNil(actual) }
        }
    }

    func testEvictionKeepsTwentyNewestComputers() throws {
        let row = try MobileVectors.section("mobile", "snapshot", "evictHosts") as! [String: Any]
        let snapshots = (row["savedAtMs"] as! [String: NSNumber]).map {
            BotsSnapshot(uid: "u", hostID: $0.key, savedAtMs: $0.value.uint64Value, bots: [])
        }
        XCTAssertEqual(BotsLogic.evictHosts(snapshots).map(\.hostID), row["expect"] as? [String])
    }

    func testAccountAndHostMismatchNeverReusesChats() {
        let prior = BotsSnapshot(uid: "another", hostID: "h", savedAtMs: 1, bots: [BotsItem(id: "b", chats: [BotsChat(id: "secret")])])
        let next = BotsLogic.applyBots(prior: prior, uid: "u", hostID: "h", bots: [BotsItem(id: "b")], nowMs: 2)
        XCTAssertNil(next.bots.first?.chats)
    }

    func testRoutesPersistAndSessionBackReturnsToTree() throws {
        let route = RemoteRoute.stack(host: "h", session: "s", title: "Chat")
        XCTAssertEqual(route, [.session(host: "h", session: "s", title: "Chat")])
        XCTAssertEqual(try JSONDecoder().decode([RemoteRoute].self, from: JSONEncoder().encode(route)), route)
        XCTAssertThrowsError(try JSONDecoder().decode([RemoteRoute].self, from: Data("[{\"bot\":{\"host\":\"h\",\"bot\":\"b\"}}]".utf8)))
    }

    private func host(_ value: [String: Any]) -> BotsLogic.Host {
        BotsLogic.Host(id: value["hostId"] as! String, name: value["name"] as? String ?? "",
            online: value["online"] as? Bool ?? false, paired: value["paired"] as? Bool ?? true,
            lastSeenMs: (value["lastSeenMs"] as? NSNumber)?.uint64Value ?? 0)
    }
    private func snapshot(_ value: Any?) -> BotsSnapshot? {
        guard let value = value as? [String: Any] else { return nil }
        return BotsSnapshot(uid: "u", hostID: value["hostId"] as! String, savedAtMs: (value["savedAtMs"] as! NSNumber).uint64Value,
            bots: (value["bots"] as! [[String: Any]]).map {
                BotsItem(id: $0["botId"] as! String, chats: ($0["chats"] as? [String])?.map { BotsChat(id: $0) })
            })
    }
    private func assertShape(_ actual: BotsSnapshot, _ expected: BotsSnapshot, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.hostID, expected.hostID, file: file, line: line)
        XCTAssertEqual(actual.savedAtMs, expected.savedAtMs, file: file, line: line)
        XCTAssertEqual(actual.bots.map(\.id), expected.bots.map(\.id), file: file, line: line)
        XCTAssertEqual(actual.bots.map { $0.chats?.map(\.id) }, expected.bots.map { $0.chats?.map(\.id) }, file: file, line: line)
    }
}
