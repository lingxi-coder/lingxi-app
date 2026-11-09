import Foundation
import XCTest
@testable import LingxiCode

/// The Friends links and push routes of docs/FRIENDS-CLIENTS.md D16, against the vectors every client loads
/// (`packages/teams/fixtures/friends-vectors.json`, `links.typed` and `links.push`).
final class FriendLinkTests: XCTestCase {
    private let uid = "Hk3xQ9vR2mT7wL1pZ8cN4aB0cD2e"
    private let code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-"

    private func rows(_ name: String) throws -> [[String: Any]] {
        var root = URL(fileURLWithPath: #filePath)
        while !FileManager.default.fileExists(atPath: root.appendingPathComponent("packages/teams/fixtures/friends-vectors.json").path), root.path != "/" { root.deleteLastPathComponent() }
        let file = Bundle(for: Self.self).url(forResource: "friends-vectors", withExtension: "json") ?? root.appendingPathComponent("packages/teams/fixtures/friends-vectors.json")
        let vectors = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        let links = try XCTUnwrap(vectors["links"] as? [String: Any])
        return try XCTUnwrap(links[name] as? [[String: Any]])
    }

    func testTypedLinksFollowTheSharedVectors() throws {
        let typed = try rows("typed")
        XCTAssertGreaterThanOrEqual(typed.count, 25, "vectors not loaded")
        for row in typed {
            let url = try XCTUnwrap(row["url"] as? String)
            let link = FriendLink(raw: url)
            guard let expect = row["expect"] as? [String: Any] else { XCTAssertNil(link, url); continue }
            switch (expect["kind"] as? String, link) {
            case let ("add", .add(code)?): XCTAssertEqual(code, expect["code"] as? String, url)
            case ("requests", .requests?): break
            case let ("chat", .chat(uid)?): XCTAssertEqual(uid, expect["uid"] as? String, url)
            default: XCTFail("\(url): expected \(expect), got \(String(describing: link))")
            }
        }
    }

    func testPushRoutesFollowTheSharedVectors() throws {
        let push = try rows("push")
        XCTAssertGreaterThanOrEqual(push.count, 20, "vectors not loaded")
        for row in push {
            let route = try XCTUnwrap(row["route"] as? String)
            let link = PushMessagePolicy.link(forRoute: route)
            if let expect = row["expect"] as? String { XCTAssertEqual(link, CanonicalDeepLink.parse(expect), route); XCTAssertNotNil(link, route) }
            else { XCTAssertNil(link, route) }
        }
    }

    func testPushOpensFriendRoutesForTheCurrentUnexpiredAccountOnly() {
        let now = Date(timeIntervalSince1970: 1)
        var payload: [AnyHashable: Any] = ["recipientUid": "A", "expiresAt": "2000", "route": "friend/requests"]
        func opened() -> FriendLink? {
            guard case .canonical(let target)? = PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: now) else { return nil }
            return FriendLink(target)
        }
        XCTAssertEqual(opened(), .requests)
        payload["route"] = "friend/chat?uid=\(uid)"
        XCTAssertEqual(opened(), .chat(uid: uid))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "B", enabled: true, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: false, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: Date(timeIntervalSince1970: 2)))
        for invalid in ["friend/add?code=\(code)", "friend/chat", "friend/chat?uid=a%40b", "friend/chat?uid=\(uid)&x=1", "friend/requests?x=1", "friend/requests\n", "friend/other"] {
            payload["route"] = invalid
            XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: now), invalid)
        }
    }

    func testFriendRoutesAreNotConfusedWithOtherRoutes() {
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://session/abc?mode=chat&uid=x"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://new?uid=x"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://team/join?code=\(code)&uid=x"))
        XCTAssertEqual(CanonicalDeepLink.parse("lingxi://friend/add?code=\(code)")?.route, "friend/add")
        XCTAssertEqual(FriendLink(raw: "https://lingxios.net/open/friend/add?code=\(code)"), .add(code: code))
        XCTAssertNil(FriendLink(raw: "lingxi://team/inbox"))
        XCTAssertNil(PushMessagePolicy.link(forRoute: "settings/account"))
        XCTAssertNotNil(PushMessagePolicy.link(forRoute: "team/inbox"))
    }
}
