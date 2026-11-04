import XCTest
@testable import LingxiCode

final class TeamsTests: XCTestCase {
    func testInvitationPreservesCaseAndRejectsAmbiguousLinks() {
        let code = "AbCdEfGhIjKlMnOpQrStUvWxYz_12345"
        for raw in [TeamInvitation.link(code), "lingxi://team/join?code=\(code)"] {
            XCTAssertEqual(TeamInvitation.code(raw), code)
            XCTAssertEqual(CanonicalDeepLink.parse(raw)?.route, "team/join")
            XCTAssertEqual(CanonicalDeepLink.parse(raw)?.params["code"], code)
            XCTAssertFalse(CanonicalDeepLink.parse(raw)!.needsContextRebind(workspace: "project.other", mode: "code"))
        }
        for raw in ["lingxi://team/join", "lingxi://team/join?code=short", "lingxi://team/join?code=\(code)&code=\(code)",
                    "lingxi://team/join?code=\(code)&workspace=secret", "lingxi://team/join?code=\(code)#payload=x",
                    "https://user:password@lingxios.net/open/team/join?code=\(code)"] {
            XCTAssertNil(TeamInvitation.code(raw), raw)
            XCTAssertNil(CanonicalDeepLink.parse(raw), raw)
        }
    }
}
