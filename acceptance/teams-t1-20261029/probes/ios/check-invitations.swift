import Foundation

@main enum CheckInvitations {
    static func main() {
        let code = "AbCdEfGhIjKlMnOpQrStUvWxYz_12345"
        for raw in [TeamInvitation.link(code), "lingxi://team/join?code=\(code)"] {
            precondition(TeamInvitation.code(raw) == code)
            let link = CanonicalDeepLink.parse(raw)!
            precondition(link.route == "team/join" && link.params["code"] == code)
            precondition(!link.needsContextRebind(workspace: "project.other", mode: "code"))
        }
        let invalid = ["lingxi://team/join", "lingxi://team/join?code=short", "lingxi://team/join?code=\(code)&code=\(code)",
                       "lingxi://team/join?code=\(code)&workspace=secret", "lingxi://team/join?code=\(code)#payload=x",
                       "https://user:password@lingxios.net/open/team/join?code=\(code)", "https://lingxios.net.evil/open/team/join?code=\(code)"]
        for raw in invalid { precondition(TeamInvitation.code(raw) == nil && CanonicalDeepLink.parse(raw) == nil) }
        precondition(CanonicalDeepLink.parse("lingxi://session/abc?code=\(code)") == nil)
        print("10 invitation cases passed against actual iOS parser sources")
    }
}
