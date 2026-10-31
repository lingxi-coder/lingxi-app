import XCTest
@testable import LingxiCode

final class ProbeEnvironmentTests: XCTestCase {
    func testErrorDiagnosticsExposeOnlyDomainsAndNumericCodes() {
        let secret = "TOKEN_AND_ERROR_MESSAGE_MUST_STAY_PRIVATE"
        let underlying = NSError(domain: NSOSStatusErrorDomain, code: -34018, userInfo: [NSLocalizedDescriptionKey: secret])
        let keychain = NSError(domain: "FIRAuthErrorDomain", code: 17995, userInfo: [
            NSLocalizedDescriptionKey: secret,
            NSLocalizedFailureReasonErrorKey: "SecItemAdd (-34018) " + secret,
            NSUnderlyingErrorKey: underlying,
        ])
        let noUnderlying = NSError(domain: "FIRAuthErrorDomain", code: 17995, userInfo: [NSLocalizedFailureReasonErrorKey: "SecItemAdd (-34018) " + secret])
        XCTAssertEqual(ProbeErrorDiagnostics.summary(noUnderlying), "FIRAuthErrorDomain / 17995 -> \(NSOSStatusErrorDomain) / -34018")
        let locked = NSError(domain: "FIRAuthErrorDomain", code: 17995, userInfo: [NSLocalizedFailureReasonErrorKey: "SecItemCopyMatching (-25308) " + secret])
        XCTAssertEqual(ProbeErrorDiagnostics.summary(locked), "FIRAuthErrorDomain / 17995 -> \(NSOSStatusErrorDomain) / -25308")
        let summary = ProbeErrorDiagnostics.summary(keychain)
        XCTAssertTrue(summary.contains("FIRAuthErrorDomain / 17995"))
        XCTAssertTrue(summary.contains("\(NSOSStatusErrorDomain) / -34018"))
        XCTAssertFalse(summary.contains(secret))
        XCTAssertFalse(summary.contains("SecItemAdd"))
        let unrelated = NSError(domain: "OtherErrorDomain", code: 17995, userInfo: [NSLocalizedFailureReasonErrorKey: "SecItemAdd (-34018) " + secret])
        XCTAssertEqual(ProbeErrorDiagnostics.summary(unrelated), "OtherErrorDomain / 17995")
    }
    func testOnlyExplicitLoopbackFixtureIsAccepted() throws {
        let environment = try ProbeEnvironment(["TEAM_TEST_BACKEND": "http://127.0.0.1:9100", "TEAM_TEST_UID": "acceptance_member"])
        XCTAssertEqual(environment.messagesURL.absoluteString, "http://127.0.0.1:9100/v1/teams/messages")
        XCTAssertEqual(URLComponents(url: environment.bootstrapURL, resolvingAgainstBaseURL: false)?.queryItems, [URLQueryItem(name: "uid", value: "acceptance_member")])
        XCTAssertEqual(environment.authPort, 9098)
        XCTAssertEqual(environment.firestorePort, 8085)
        XCTAssertEqual(ProbeEnvironment.projectID, "demo-lingxi-teams-t0")
        for backend in ["https://lingxios.net", "https://127.0.0.1:9100", "http://example.com:9100", "http://user@127.0.0.1:9100", "http://127.0.0.1:9100/api", "http://127.0.0.1:9100?token=x", "http://127.0.0.1:9100#x"] {
            XCTAssertThrowsError(try ProbeEnvironment(["TEAM_TEST_BACKEND": backend, "TEAM_TEST_UID": "acceptance_member"]))
        }
        XCTAssertThrowsError(try ProbeEnvironment([:]))
        XCTAssertThrowsError(try ProbeEnvironment(["TEAM_TEST_BACKEND": "http://127.0.0.1:9100", "TEAM_TEST_UID": "acceptance_member", "TEAM_TEST_AUTH_PORT": "0"]))
    }
}
