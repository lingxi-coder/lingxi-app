import Foundation
import Security
import XCTest
@testable import LingxiCode

final class LocalAppAgentHostLifecycleTests: XCTestCase {
    func testStaleHandoffReasonDistinguishesReplacementAndRevisionChange() {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let replacement = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-b")
        XCTAssertEqual(LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake, currentWake: replacement, currentRevision: 1
        ), .requestReplaced)
        XCTAssertEqual(LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake, currentWake: wake, currentRevision: 2
        ), .releaseChanged)
        XCTAssertEqual(LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake, currentWake: nil, currentRevision: nil
        ), .acquireFailure)
    }

    func testExistingTaskOperationsReuseTheAvailableHost() {
        for operation in [
            "tasks.list", "eventsNative", "tasks.cancel", "wake.clear",
            "interactions", "answer", "settings"
        ] {
            let request = #"{"op":"\#(operation)"}"#
            XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: request), .available, operation)
        }

        for operation in ["wake.reserve", "startNative", "catalog", "connectMcp", "configure"] {
            let request = #"{"op":"\#(operation)"}"#
            XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: request), .currentConfiguration, operation)
        }
        XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: #"{"op":"configure","settings":null}"#), .available)
        XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: #"{"op":"registerAuthoring"}"#), .none)
        XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: "invalid JSON"), .currentConfiguration)
    }

    func testKeychainReadFailureIsNotADeletedValue() throws {
        XCTAssertNil(try Keychain.decodeRead(status: errSecItemNotFound, data: nil))
        XCTAssertNil(try Keychain.decodeRead(status: errSecSuccess, data: Data()))
        XCTAssertEqual(try Keychain.decodeRead(status: errSecSuccess, data: Data("fixture".utf8)), "fixture")
        XCTAssertThrowsError(try Keychain.decodeRead(status: errSecInteractionNotAllowed, data: nil)) { error in
            XCTAssertEqual(error as? Keychain.ReadError, .status(errSecInteractionNotAllowed))
        }
        XCTAssertThrowsError(try Keychain.decodeRead(status: errSecSuccess, data: nil)) { error in
            XCTAssertEqual(error as? Keychain.ReadError, .invalidData)
        }
        XCTAssertThrowsError(try Keychain.decodeRead(status: errSecSuccess, data: Data([0xff]))) { error in
            XCTAssertEqual(error as? Keychain.ReadError, .invalidData)
        }
    }
}
