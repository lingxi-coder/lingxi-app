import Foundation
import XCTest
@testable import LingxiCode

final class TeamShareLinksTests: XCTestCase {
    private func conversation(_ id: String, _ kind: String, _ participants: [String]) -> TeamMessageConversation {
        TeamMessageConversation(id: id, kind: kind, title: "", participants: participants, lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0)
    }
    private func card(_ id: String) throws -> TeamBotCard {
        let json = #"{"sharedBotId":"\#(id)","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}"#
        return try JSONDecoder().decode(TeamBotCard.self, from: Data(json.utf8))
    }

    func testLinksMatchTheWebDesktopAndAndroidOnes() {
        XCTAssertEqual(TeamShareLinks.link(teamID: "design-team", conversation: conversation("c-1", "group", ["user:a", "user:b"]))?.absoluteString,
                       "https://lingxios.net/open/team/chat?conversation=c-1&team=design-team")
        XCTAssertEqual(TeamShareLinks.link(teamID: "design-team", conversation: conversation("dm", "bot_dm", ["user:a", "shared_bot:helper_01"]))?.absoluteString,
                       "https://lingxios.net/open/team/bot?bot=helper_01&team=design-team")
        XCTAssertEqual(TeamShareLinks.link(teamID: "f_pair", conversation: conversation("friend_chat", "dm", ["user:a", "user:b"]))?.absoluteString,
                       "https://lingxios.net/open/team/chat?conversation=friend_chat&team=f_pair")
        XCTAssertNil(TeamShareLinks.link(teamID: "f_pair", conversation: conversation("dm", "bot_dm", ["user:a", "shared_bot:helper_01"])), "a friendship's Bot chat")
        XCTAssertNil(TeamShareLinks.link(teamID: "design-team", conversation: conversation("dm", "bot_dm", ["user:a"])))
        XCTAssertNil(TeamShareLinks.link(teamID: "design team", conversation: conversation("c", "group", [])))
    }

    func testRequestsNameTheirTeamAndLeaveFriendshipsToFriends() throws {
        XCTAssertEqual(TeamShareLinks.Request(try XCTUnwrap(CanonicalDeepLink.parse("lingxi://team/chat?team=design&conversation=c1"))), .conversation(teamID: "design", conversationID: "c1"))
        XCTAssertEqual(TeamShareLinks.Request(try XCTUnwrap(CanonicalDeepLink.parse("lingxi://team/bot?team=design&bot=helper"))), .bot(teamID: "design", botID: "helper"))
        XCTAssertNil(TeamShareLinks.Request(try XCTUnwrap(CanonicalDeepLink.parse("lingxi://team/chat?team=f_pair&conversation=friend_chat"))))
        XCTAssertNil(TeamShareLinks.Request(try XCTUnwrap(CanonicalDeepLink.parse("lingxi://team/bot?team=f_pair&bot=helper"))), "a pair team's Bot link is Friends' too")
        XCTAssertNil(TeamShareLinks.Request(try XCTUnwrap(CanonicalDeepLink.parse("lingxi://team/inbox"))))
    }

    func testALinkOpensItsOwnTeamOnlyAndNeverCreatesABotChat() throws {
        let chat = TeamShareLinks.Request.conversation(teamID: "design", conversationID: "c1")
        let bot = TeamShareLinks.Request.bot(teamID: "design", botID: "helper")
        func step(_ request: TeamShareLinks.Request, uid: String? = "me", listed: Bool = true, teams: [String] = ["design", "other"], selected: String = "design",
                  cache: String? = "design", server: Bool = true, botsLoaded: Bool = true, bots: [TeamBotCard] = [], conversations: [TeamMessageConversation] = []) -> TeamShareLinks.Step {
            TeamShareLinks.step(request, uid: uid, listed: listed, teamIDs: teams, selectedTeamID: selected, cacheTeamID: cache, directoryFromServer: server, botsLoaded: botsLoaded, bots: bots, conversations: conversations)
        }
        let c1 = conversation("c1", "group", ["user:me"])
        XCTAssertEqual(step(chat, uid: nil), .wait)
        XCTAssertEqual(step(chat, listed: false), .wait)
        XCTAssertEqual(step(chat, teams: ["other"]), .unavailable, "never another team")
        XCTAssertEqual(step(chat, selected: "other"), .selectTeam)
        XCTAssertEqual(step(chat, cache: "other"), .wait)
        XCTAssertEqual(step(chat, server: false, conversations: [c1]), .wait, "only the server's directory decides")
        XCTAssertEqual(step(chat, conversations: [c1]), .open("c1"))
        XCTAssertEqual(step(chat), .unavailable, "an unseen conversation says so instead of a screen that closes itself")
        XCTAssertEqual(step(bot, botsLoaded: false), .wait)
        XCTAssertEqual(step(bot), .unavailable, "a Bot no longer shared")
        let helper = try card("helper")
        let theirs = conversation("dm2", "bot_dm", ["user:them", "shared_bot:helper"])
        XCTAssertEqual(step(bot, server: false, bots: [helper]), .wait, "a Bot list before the directory does not decide on an empty one")
        XCTAssertEqual(step(bot, bots: [helper], conversations: [theirs]), .details)
        XCTAssertEqual(step(bot, bots: [helper], conversations: [theirs, conversation("dm1", "bot_dm", ["user:me", "shared_bot:helper"])]), .open("dm1"))
    }
}
