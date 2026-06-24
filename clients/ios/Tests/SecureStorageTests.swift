// SecureStorageTests.swift — round-trip for the native Keychain `SecureStorageImpl`.
//
// `SecureStorageImpl` is the production secure store the app hands to
// `buildIosEngine` (OAuth `/login` token persistence rides it; it is also what
// flips the engine's `oauth_supported` true). This proves the trait contract:
// store overwrites, retrieve returns the EXACT bytes, list enumerates accounts,
// delete removes (and deleting a missing entry is not an error).
//
// CAVEATS (run on a real device, or an app-hosted test target):
//   * Keychain access from a SIMULATOR unit test can return
//     errSecMissingEntitlement (-34018) unless the test runs in an app host
//     (TEST_HOST) with keychain-sharing entitlements. On a real device it works.
//   * If the Xcode test target lists files explicitly (not glob/xcodegen),
//     add this file to the test target's membership.

import XCTest

@testable import LingxiCode

#if canImport(Security)
    final class SecureStorageTests: XCTestCase {
        func testKeychainRoundtrip() async throws {
            let store = SecureStorageImpl()
            // Unique service so the test is isolated and self-cleaning.
            let service = "lingxi.securestore.test.\(UUID().uuidString)"
            let account = "default"
            let secret = Data("oauth-token-αβγ-🔐".utf8)
            defer { Task { try? await store.delete(service: service, account: account) } }

            // Absent → nil (missing key is distinct from an error).
            let before = try await store.retrieve(service: service, account: account)
            XCTAssertNil(before)

            // Store → retrieve the EXACT bytes back.
            try await store.store(service: service, account: account, blob: secret)
            let got = try await store.retrieve(service: service, account: account)
            XCTAssertEqual(got, secret)

            // List → contains the account.
            let accounts = try await store.list(service: service)
            XCTAssertTrue(accounts.contains(account))

            // Overwrite → replaces the prior value.
            let rotated = Data("rotated-token".utf8)
            try await store.store(service: service, account: account, blob: rotated)
            let got2 = try await store.retrieve(service: service, account: account)
            XCTAssertEqual(got2, rotated)

            // Delete → gone.
            try await store.delete(service: service, account: account)
            let after = try await store.retrieve(service: service, account: account)
            XCTAssertNil(after)

            // Deleting a non-existent entry is not an error (trait contract).
            try await store.delete(service: service, account: "never-existed")
        }
    }
#endif
