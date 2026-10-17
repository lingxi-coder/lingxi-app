import XCTest
@testable import LingxiCode

final class PushRegistrationTests: XCTestCase {
    func testInstallationIdentityRoundTripAndIndependentSecrets() throws {
        var identity = PushInstallationIdentity.create()
        identity.revision = 42
        identity.mayBeBound = true
        XCTAssertEqual(identity.secret.count, 64)
        XCTAssertNotNil(UUID(uuidString: identity.id))
        XCTAssertEqual(try JSONDecoder().decode(PushInstallationIdentity.self, from: JSONEncoder().encode(identity)), identity)
        XCTAssertNotEqual(identity.secret, PushInstallationIdentity.create().secret)
    }

    func testRemoteMessageMustMatchAccountPermissionAndExpiry() {
        let message: [AnyHashable: Any] = ["recipientUid": "user-a", "expiresAt": "2000"]
        let now = Date(timeIntervalSince1970: 1)
        XCTAssertTrue(PushMessagePolicy.isRemote(message))
        XCTAssertTrue(PushMessagePolicy.accepts(message, uid: "user-a", enabled: true, now: now))
        XCTAssertFalse(PushMessagePolicy.accepts(message, uid: nil, enabled: true, now: now))
        XCTAssertFalse(PushMessagePolicy.accepts(message, uid: "user-b", enabled: true, now: now))
        XCTAssertFalse(PushMessagePolicy.accepts(message, uid: "user-a", enabled: false, now: now))
        XCTAssertFalse(PushMessagePolicy.accepts(message, uid: "user-a", enabled: true, now: Date(timeIntervalSince1970: 2)))
        XCTAssertFalse(PushMessagePolicy.accepts(["recipientUid": "user-a", "expiresAt": "nan"], uid: "user-a", enabled: true, now: now))
        XCTAssertFalse(PushMessagePolicy.isRemote(["cronRunId": "local"]))
    }
}
