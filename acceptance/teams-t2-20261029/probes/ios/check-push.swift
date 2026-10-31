import Foundation

enum LingxiAppAction { case canonical(CanonicalDeepLink) }
@main struct CheckTeamPush {
    static func main() {
        let now = Date(timeIntervalSince1970: 1000)
        let info: [AnyHashable: Any] = ["recipientUid": "synthetic-user", "expiresAt": "1001000", "route": "team/inbox"]
        guard case .canonical(let target) = PushMessagePolicy.remoteControlAction(info, uid: "synthetic-user", enabled: true, now: now) else { preconditionFailure("Team inbox rejected") }
        precondition(target.route == "team/inbox" && !target.needsContextRebind(workspace: "other", mode: "code"))
        precondition(PushMessagePolicy.remoteControlAction(info, uid: "other-user", enabled: true, now: now) == nil)
        precondition(PushMessagePolicy.remoteControlAction(info, uid: "synthetic-user", enabled: false, now: now) == nil)
        precondition(PushMessagePolicy.remoteControlAction(info, uid: "synthetic-user", enabled: true, now: now.addingTimeInterval(10)) == nil)
        for route in ["team/inbox?workspace=x", "team/inbox#payload=%7B%7D", "team/join", "new"] {
            var invalid = info; invalid["route"] = route
            precondition(PushMessagePolicy.remoteControlAction(invalid, uid: "synthetic-user", enabled: true, now: now) == nil)
        }
        print("Team push: 9 route/account/expiry checks passed")
    }
}
