import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class ProviderAccountRepositoryTests: XCTestCase {
    private func snapshot(_ revision: UInt64, accounts: [String] = ["a", "b"], needsAuth: String? = nil) throws -> String {
        let rows: [[String: Any]] = accounts.enumerated().map { priority, id in
            ["accountId": id, "providerId": "openai", "displayName": "Account \(id)",
             "authKind": "oauth", "executionBindingId": "openai-chatgpt", "enabled": true,
             "userPriority": priority, "configRevision": revision, "identityGeneration": 1]
        }
        let health: [[String: Any]] = accounts.map { id in
            ["accountId": id, "identityGeneration": 1,
             "status": id == needsAuth ? "needs_auth" : "ready", "healthRevision": revision]
        }
        return String(decoding: try JSONSerialization.data(withJSONObject: [
            "storeScope": "test-scope", "configRevision": revision, "accounts": rows, "health": health, "modelEntitlements": [], "modelAvailability": [], "scopedHealth": [],
        ]), as: UTF8.self)
    }

    private func cooldownSnapshot(recovered: Bool) throws -> String {
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(snapshot(4).utf8)) as? [String: Any])
        object["modelAvailability"] = [["providerId": "openai", "executionBindingId": "openai-chatgpt",
            "modelId": "fixture-model", "eligibleAccountIds": recovered ? ["a"] : [],
            "confirmedAccountIds": [], "unverifiedAccountIds": recovered ? ["a"] : [], "forbiddenAccountIds": []]]
        if !recovered {
            object["health"] = [["accountId": "a", "identityGeneration": 1, "status": "cooling_down",
                "cooldownUntilMs": 6000, "healthRevision": 1]]
            object["modelEntitlements"] = [["accountId": "a", "identityGeneration": 1,
                "modelId": "fixture-model", "status": "forbidden", "observedAtMs": 1000, "retryAtMs": 5000]]
            object["scopedHealth"] = [["accountId": "a", "identityGeneration": 1,
                "scopeKind": "endpoint", "scopeId": "fixture-endpoint", "unavailableUntilMs": 4000, "reason": "cooldown"]]
        }
        return String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
    }

    func testNextHostRecoveryDeadlineRefreshesProjectionWithoutAnyCommand() async throws {
        var now: UInt64 = 1000
        var release: CheckedContinuation<Void, Never>?
        var waits: [UInt64] = []
        let repository = ProviderAccountRepository(nowMilliseconds: { now }, recoveryDelay: { milliseconds in
            waits.append(milliseconds)
            await withCheckedContinuation { release = $0 }
        })
        let cooling = try cooldownSnapshot(recovered: false)
        let recovered = try cooldownSnapshot(recovered: true)
        var reads = 0
        var commands = 0
        repository.configure(load: { reads += 1; return reads == 1 ? cooling : recovered },
            command: { _, _ in commands += 1; return recovered })
        await repository.refresh()
        XCTAssertFalse(repository.modelIsAvailable(reference: "openai-chatgpt/fixture-model"))
        while release == nil { await Task.yield() }
        XCTAssertEqual(waits, [3000], "The earliest Host scope, entitlement, or account deadline wins")
        now = 4000
        release?.resume(); release = nil
        while reads < 2 { await Task.yield() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertTrue(repository.modelIsAvailable(reference: "openai-chatgpt/fixture-model"))
        XCTAssertEqual(reads, 2)
        XCTAssertEqual(commands, 0)
    }

    func testBackgroundCancelsDeadlineAndForegroundReloadsHostMetadata() async throws {
        var now: UInt64 = 1000
        var release: CheckedContinuation<Void, Never>?
        let repository = ProviderAccountRepository(nowMilliseconds: { now }, recoveryDelay: { _ in
            await withCheckedContinuation { release = $0 }
        })
        let cooling = try cooldownSnapshot(recovered: false)
        let recovered = try cooldownSnapshot(recovered: true)
        var reads = 0
        repository.configure(load: { reads += 1; return reads == 1 ? cooling : recovered },
            command: { _, _ in XCTFail("Recovery is read-only"); return recovered })
        await repository.refresh()
        while release == nil { await Task.yield() }
        repository.setForeground(false)
        now = 6000
        release?.resume(); release = nil
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(reads, 1)
        repository.setForeground(true)
        while reads < 2 { await Task.yield() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertTrue(repository.modelIsAvailable(reference: "openai-chatgpt/fixture-model"))
    }

    func testOldHostRecoveryTimerCannotReadReplacementHost() async throws {
        var release: CheckedContinuation<Void, Never>?
        let repository = ProviderAccountRepository(nowMilliseconds: { 1000 }, recoveryDelay: { _ in
            await withCheckedContinuation { release = $0 }
        })
        let cooling = try cooldownSnapshot(recovered: false)
        var nextObject = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(snapshot(1, accounts: ["next-host"]).utf8)) as? [String: Any])
        nextObject["modelAvailability"] = [["providerId": "openai", "executionBindingId": "openai-chatgpt", "modelId": "fixture-model",
            "eligibleAccountIds": ["next-host"], "confirmedAccountIds": [], "unverifiedAccountIds": ["next-host"], "forbiddenAccountIds": []]]
        let nextHost = String(decoding: try JSONSerialization.data(withJSONObject: nextObject), as: UTF8.self)
        repository.configure(load: { cooling }, command: { _, _ in cooling })
        await repository.refresh()
        while release == nil { await Task.yield() }
        var nextReads = 0
        repository.configure(load: { nextReads += 1; return nextHost }, command: { _, _ in nextHost })
        await repository.refresh()
        release?.resume(); release = nil
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(nextReads, 1)
        XCTAssertEqual(repository.snapshot.accounts.map(\.accountId), ["next-host"])
    }

    func testExpiredHostDeadlineUsesBoundedReadOnlyFallbackInsteadOfLatchingOrSpinning() async throws {
        var now: UInt64 = 6000
        var release: CheckedContinuation<Void, Never>?
        var waits: [UInt64] = []
        let repository = ProviderAccountRepository(nowMilliseconds: { now }, recoveryDelay: { milliseconds in
            waits.append(milliseconds)
            await withCheckedContinuation { release = $0 }
        })
        let expired = try cooldownSnapshot(recovered: false)
        let recovered = try cooldownSnapshot(recovered: true)
        var reads = 0
        repository.configure(load: { reads += 1; return reads == 1 ? expired : recovered },
            command: { _, _ in XCTFail("Recovery never issues provider commands"); return recovered })
        await repository.refresh()
        while release == nil { await Task.yield() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(reads, 1)
        XCTAssertEqual(waits, [30_000])
        now += 30_000
        release?.resume(); release = nil
        while reads < 2 { await Task.yield() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertTrue(repository.modelIsAvailable(reference: "openai-chatgpt/fixture-model"))
        XCTAssertEqual(waits, [30_000])
    }

    func testFutureHostDeadlineWaitIsBoundedAcrossClockSkew() async throws {
        var release: CheckedContinuation<Void, Never>?
        var waits: [UInt64] = []
        let repository = ProviderAccountRepository(nowMilliseconds: { 1000 }, recoveryDelay: { milliseconds in
            waits.append(milliseconds)
            await withCheckedContinuation { release = $0 }
        })
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(cooldownSnapshot(recovered: false).utf8)) as? [String: Any])
        object["health"] = []
        object["modelEntitlements"] = []
        object["scopedHealth"] = [["accountId": "a", "identityGeneration": 1,
            "scopeKind": "endpoint", "scopeId": "fixture-endpoint", "unavailableUntilMs": 900_000, "reason": "cooldown"]]
        let json = String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
        repository.configure(load: { json }, command: { _, _ in XCTFail("Recovery is metadata-only"); return json })
        await repository.refresh()
        while release == nil { await Task.yield() }
        XCTAssertEqual(waits, [30_000])
        repository.setForeground(false)
        release?.resume(); release = nil
    }

    func testRecoveryIgnoresDeadlinesFromAnOldAccountGeneration() async throws {
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(cooldownSnapshot(recovered: true).utf8)) as? [String: Any])
        object["scopedHealth"] = [["accountId": "a", "identityGeneration": 2,
            "scopeKind": "model", "scopeId": "fixture-model", "unavailableUntilMs": 2000, "reason": "cooldown"]]
        let json = String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
        var waits = 0
        let repository = ProviderAccountRepository(nowMilliseconds: { 1000 }, recoveryDelay: { _ in waits += 1 })
        repository.configure(load: { json }, command: { _, _ in json })
        await repository.refresh()
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(waits, 0)
    }

    func testCatalogPublicationRefreshRejectsOlderSameRevisionSnapshot() async throws {
        let repository = ProviderAccountRepository()
        let oldJSON = try snapshot(1)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(oldJSON.utf8)) as? [String: Any])
        object["modelAvailability"] = [["providerId": "openai", "executionBindingId": "openai-chatgpt",
            "modelId": "new-model", "eligibleAccountIds": ["a"], "confirmedAccountIds": [],
            "unverifiedAccountIds": ["a"], "forbiddenAccountIds": []]]
        let newJSON = String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
        var oldRead: CheckedContinuation<String, Never>?
        var reads = 0
        repository.configure(load: {
            reads += 1
            if reads == 1 { return await withCheckedContinuation { oldRead = $0 } }
            return newJSON
        }, command: { _, _ in XCTFail("Catalog changes are read-only"); return newJSON })
        let first = Task { await repository.refresh() }
        while oldRead == nil { await Task.yield() }
        repository.refreshAfterCatalogPublication()
        for _ in 0..<100 where reads < 2 { await Task.yield() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertTrue(repository.modelIsAvailable(reference: "openai-chatgpt/new-model"))
        oldRead?.resume(returning: oldJSON)
        await first.value
        XCTAssertEqual(repository.snapshot.configRevision, 1)
        XCTAssertTrue(repository.modelIsAvailable(reference: "openai-chatgpt/new-model"),
            "A pre-publication response with the same configuration revision cannot overwrite the fresh projection")
    }

    func testCatalogPublicationRefreshCannotReadReplacementHost() async throws {
        let repository = ProviderAccountRepository()
        let json = try snapshot(1)
        var originalReads = 0
        var replacementReads = 0
        repository.configure(load: { originalReads += 1; return json }, command: { _, _ in json })
        repository.refreshAfterCatalogPublication()
        repository.configure(load: { replacementReads += 1; return json }, command: { _, _ in json })
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(originalReads, 0)
        XCTAssertEqual(replacementReads, 0, "A queued old catalog event cannot read a replacement Host")
    }

    private func deviceGrant(account: String = "a", flow: String = "flow-a") throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: [
            "mode": "device_code", "flow_id": flow, "account_id": account, "user_code": "ABCD-EFGH",
            "verification_url": "https://auth.openai.com/codex/device", "interval_seconds": 5, "expires_in_seconds": 900,
        ]), as: UTF8.self)
    }

    private func completedDevicePoll(_ snapshotJSON: String) throws -> String {
        let snapshot = try JSONSerialization.jsonObject(with: Data(snapshotJSON.utf8))
        return String(decoding: try JSONSerialization.data(withJSONObject: [
            "status": "completed", "retry_after_seconds": 0, "snapshot": snapshot,
        ]), as: UTF8.self)
    }

    func testDeviceCodePublishesCodeAndPollsAtHostIntervalsUntilExactAccountCompletes() async throws {
        var release: CheckedContinuation<Void, Never>?
        var delays: [UInt64] = []
        let repository = ProviderAccountRepository(devicePollDelay: { seconds in
            delays.append(seconds)
            await withCheckedContinuation { release = $0 }
        })
        let initial = try snapshot(4)
        let completed = try completedDevicePoll(snapshot(5))
        let grant = try deviceGrant()
        var polls = 0
        repository.configure(load: { initial }, command: { raw, credential in
            XCTAssertNil(credential)
            let command = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any])
            XCTAssertEqual(command["accountId"] as? String, "a")
            XCTAssertEqual((command["expectedRevision"] as? NSNumber)?.uint64Value, 4)
            if command["action"] as? String == "begin_oauth" { return grant }
            XCTAssertEqual(command["action"] as? String, "poll_oauth")
            XCTAssertEqual(command["flowId"] as? String, "flow-a")
            polls += 1
            return polls == 1 ? #"{"status":"pending","retry_after_seconds":7}"# : completed
        })
        await repository.refresh()
        let started = await repository.perform(.init(action: "begin_oauth", accountId: "a"))
        XCTAssertTrue(started)
        XCTAssertEqual(repository.deviceFlow?.userCode, "ABCD-EFGH")
        XCTAssertEqual(repository.deviceFlow?.accountId, "a")
        while release == nil { await Task.yield() }
        release?.resume(); release = nil
        while polls < 1 || release == nil { await Task.yield() }
        release?.resume(); release = nil
        while repository.deviceFlow != nil { await Task.yield() }
        XCTAssertEqual(delays, [5, 7])
        XCTAssertEqual(repository.snapshot.configRevision, 5)
        XCTAssertNil(repository.errorMessage)
    }

    func testDevicePollCompletionAfterHostRebindCannotRestoreOldAccountState() async throws {
        let repository = ProviderAccountRepository(devicePollDelay: { _ in await Task.yield() })
        let initial = try snapshot(4)
        let nextHost = try snapshot(1, accounts: ["new-host"])
        let grant = try deviceGrant()
        let completed = try completedDevicePoll(snapshot(99))
        var pending: CheckedContinuation<String, Never>?
        var cancelled: [String] = []
        repository.configure(load: { initial }, command: { raw, _ in
            let command = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any])
            switch command["action"] as? String {
            case "begin_oauth": return grant
            case "poll_oauth": return await withCheckedContinuation { pending = $0 }
            case "cancel_oauth": cancelled.append(command["accountId"] as? String ?? ""); return initial
            default: throw ProviderAccountError.invalidResponse
            }
        })
        await repository.refresh()
        _ = await repository.perform(.init(action: "begin_oauth", accountId: "a"))
        while pending == nil { await Task.yield() }
        repository.configure(load: { nextHost }, command: { _, _ in nextHost })
        await repository.refresh()
        pending?.resume(returning: completed)
        for _ in 0..<20 { await Task.yield() }
        XCTAssertEqual(repository.snapshot.accounts.map(\.accountId), ["new-host"])
        XCTAssertNil(repository.deviceFlow)
        XCTAssertEqual(cancelled, ["a"])
    }

    func testLateDeviceGrantIsCancelledThroughItsOriginalHost() async throws {
        let repository = ProviderAccountRepository()
        let initial = try snapshot(4)
        let nextHost = try snapshot(1, accounts: ["new-host"])
        let grant = try deviceGrant()
        var pending: CheckedContinuation<String, Never>?
        var cancelled = false
        repository.configure(load: { initial }, command: { raw, _ in
            let command = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any])
            if command["action"] as? String == "cancel_oauth" {
                XCTAssertEqual(command["accountId"] as? String, "a")
                XCTAssertEqual(command["flowId"] as? String, "flow-a")
                cancelled = true
                return initial
            }
            return await withCheckedContinuation { pending = $0 }
        })
        await repository.refresh()
        let operation = Task { await repository.perform(.init(action: "begin_oauth", accountId: "a")) }
        while pending == nil { await Task.yield() }
        repository.configure(load: { nextHost }, command: { _, _ in nextHost })
        await repository.refresh()
        pending?.resume(returning: grant)
        let accepted = await operation.value
        XCTAssertFalse(accepted)
        XCTAssertTrue(cancelled)
        XCTAssertNil(repository.deviceFlow)
        XCTAssertEqual(repository.snapshot.accounts.map(\.accountId), ["new-host"])
    }

    func testScopedHealthRemainsBoundToExactAccountIdentity() async throws {
        let repository = ProviderAccountRepository()
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(snapshot(4).utf8)) as? [String: Any])
        json["scopedHealth"] = [
            ["accountId": "a", "identityGeneration": 1, "scopeKind": "model", "scopeId": "model-one", "reason": "rate_limited", "unavailableUntilMs": 5000],
            ["accountId": "b", "identityGeneration": 1, "scopeKind": "model", "scopeId": "other-account", "reason": "rate_limited"],
            ["accountId": "a", "identityGeneration": 0, "scopeKind": "endpoint", "scopeId": "old-identity", "reason": "network"],
        ]
        let response = String(decoding: try JSONSerialization.data(withJSONObject: json), as: UTF8.self)
        repository.configure(load: { response }, command: nil)
        await repository.refresh()
        let account = try XCTUnwrap(repository.snapshot.accounts.first { $0.accountId == "a" })
        XCTAssertEqual(repository.scopedHealth(for: account).map(\.scopeId), ["model-one"])
        XCTAssertEqual(repository.scopedHealth(for: account).first?.unavailableUntilMs, 5000)
    }

    func testModelEntitlementsRemainScopedToAccountIdentity() async throws {
        let repository = ProviderAccountRepository()
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(snapshot(4).utf8)) as? [String: Any])
        json["modelEntitlements"] = [
            ["accountId": "a", "identityGeneration": 1, "modelId": "gpt-a", "status": "confirmed", "observedAtMs": 10],
            ["accountId": "b", "identityGeneration": 1, "modelId": "gpt-a", "status": "forbidden", "observedAtMs": 11],
            ["accountId": "a", "identityGeneration": 0, "modelId": "stale", "status": "confirmed", "observedAtMs": 1],
        ]
        let response = String(decoding: try JSONSerialization.data(withJSONObject: json), as: UTF8.self)
        repository.configure(load: { response }, command: nil)
        await repository.refresh()
        let account = try XCTUnwrap(repository.snapshot.accounts.first { $0.accountId == "a" })
        XCTAssertEqual(repository.modelEntitlements(for: account).map(\.modelId), ["gpt-a"])
        let other = try XCTUnwrap(repository.snapshot.accounts.first { $0.accountId == "b" })
        XCTAssertEqual(repository.modelEntitlements(for: other).first?.status, "forbidden")
    }

    func testAvailableModelUnionUsesHostProjectionWithoutInferringAuthorization() async throws {
        let repository = ProviderAccountRepository()
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(snapshot(4).utf8)) as? [String: Any])
        json["modelAvailability"] = [
            ["providerId": "openai", "executionBindingId": "openai-chatgpt", "modelId": "gpt-a",
             "eligibleAccountIds": ["b"], "confirmedAccountIds": [], "unverifiedAccountIds": ["b"], "forbiddenAccountIds": ["a"]],
            ["providerId": "openai", "executionBindingId": "openai-chatgpt", "modelId": "gpt-b",
             "eligibleAccountIds": [], "confirmedAccountIds": [], "unverifiedAccountIds": [], "forbiddenAccountIds": ["a", "b"]],
        ]
        let response = String(decoding: try JSONSerialization.data(withJSONObject: json), as: UTF8.self)
        repository.configure(load: { response }, command: nil)
        await repository.refresh()
        XCTAssertTrue(repository.modelIsAvailable(reference: "openai-chatgpt/gpt-a"))
        XCTAssertFalse(repository.modelIsAvailable(reference: "openai-chatgpt/gpt-b"))
        XCTAssertFalse(repository.modelIsAvailable(reference: "openai/gpt-a"))
        XCTAssertFalse(repository.modelIsAvailable(reference: "openai-chatgpt/catalog-only"))
    }

    func testRefreshIsReadOnlyAndHealthDoesNotReorderAccounts() async throws {
        let repository = ProviderAccountRepository()
        let first = try snapshot(4, needsAuth: "a")
        var reads = 0
        var mutations = 0
        repository.configure(load: { reads += 1; return first }, command: { _, _ in mutations += 1; return first })
        for _ in 0..<3 { await repository.refresh() }
        XCTAssertEqual(reads, 3)
        XCTAssertEqual(mutations, 0)
        XCTAssertEqual(repository.accounts(providerId: "openai").map(\.accountId), ["a", "b"])
        XCTAssertEqual(repository.health(for: try XCTUnwrap(repository.snapshot.accounts.first))?.status, "needs_auth")
    }

    func testCredentialsUseSeparateArgumentAndAuthoritativeRevision() async throws {
        let repository = ProviderAccountRepository()
        let first = try snapshot(4)
        let updated = try snapshot(5)
        var request: [String: Any] = [:]
        var receivedCredential: String?
        repository.configure(load: { first }, command: { json, credential in
            request = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
            XCTAssertFalse(json.contains("test-secret"))
            receivedCredential = credential
            return updated
        })
        await repository.refresh()
        let succeeded = await repository.perform(.init(action: "set_credential", expectedRevision: 0, accountId: "a"), credential: "test-secret")
        XCTAssertTrue(succeeded)
        XCTAssertEqual((request["expectedRevision"] as? NSNumber)?.uint64Value, 4)
        XCTAssertEqual(request["accountId"] as? String, "a")
        let operationID = try XCTUnwrap((request["operationId"] as? NSNumber)?.uint64Value)
        XCTAssertGreaterThan(operationID, 0)
        XCTAssertLessThanOrEqual(operationID, 9_007_199_254_740_991)
        XCTAssertEqual(receivedCredential, "test-secret")
        XCTAssertEqual(repository.snapshot.configRevision, 5)
    }

    func testMutationResultFromOldHostCannotRestoreAccountsAfterRebind() async throws {
        let repository = ProviderAccountRepository()
        let old = try snapshot(10)
        let new = try snapshot(1, accounts: ["new-host-account"])
        var pending: CheckedContinuation<String, Never>?
        repository.configure(load: { old }, command: { _, _ in
            await withCheckedContinuation { pending = $0 }
        })
        await repository.refresh()
        let operation = Task { await repository.perform(.init(action: "remove", accountId: "a")) }
        while pending == nil { await Task.yield() }
        repository.configure(load: { new }, command: { _, _ in new })
        await repository.refresh()
        pending?.resume(returning: old)
        let succeeded = await operation.value
        XCTAssertFalse(succeeded)
        XCTAssertEqual(repository.snapshot.accounts.map(\.accountId), ["new-host-account"])
        XCTAssertFalse(repository.busy)
    }

    func testLateReadCannotOverwriteCommittedReorder() async throws {
        let repository = ProviderAccountRepository()
        let old = try snapshot(4)
        let updated = try snapshot(5, accounts: ["b", "a"])
        var read: CheckedContinuation<String, Never>?
        var loads = 0
        repository.configure(load: {
            loads += 1
            if loads == 1 { return await withCheckedContinuation { read = $0 } }
            return updated
        }, command: { _, _ in updated })
        let refresh = Task { await repository.refresh() }
        while read == nil { await Task.yield() }
        let succeeded = await repository.perform(.init(action: "reorder", providerId: "openai", accountIds: ["b", "a"]))
        XCTAssertTrue(succeeded)
        read?.resume(returning: old)
        await refresh.value
        XCTAssertEqual(repository.accounts(providerId: "openai").map(\.accountId), ["b", "a"])
        XCTAssertEqual(repository.snapshot.configRevision, 5)
    }

    func testRevisionFailureRefreshesWithoutRetryingMutation() async throws {
        let repository = ProviderAccountRepository()
        let first = try snapshot(4)
        let updated = try snapshot(5, accounts: ["b"])
        var loads = 0
        var mutations = 0
        repository.configure(load: { loads += 1; return loads == 1 ? first : updated }, command: { _, _ in
            mutations += 1
            throw NSError(domain: "AccountRevisionConflict", code: 1)
        })
        await repository.refresh()
        let succeeded = await repository.perform(.init(action: "remove", accountId: "a"))
        XCTAssertFalse(succeeded)
        XCTAssertEqual(mutations, 1)
        XCTAssertEqual(loads, 2)
        XCTAssertEqual(repository.snapshot.accounts.map(\.accountId), ["b"])
        XCTAssertNotNil(repository.errorMessage)
        XCTAssertFalse(repository.busy)
    }

    func testSubscriptionBillingConsentIsExplicitAndRevisionFenced() async throws {
        let repository = ProviderAccountRepository()
        let first = try snapshot(4)
        var updated = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(try snapshot(5).utf8)) as? [String: Any])
        updated["providerPolicies"] = ["openai": ["allowSubscriptionToMetered": true]]
        let updatedJSON = String(decoding: try JSONSerialization.data(withJSONObject: updated), as: UTF8.self)
        var reads = 0
        repository.configure(load: { reads += 1; return reads == 1 ? first : updatedJSON }, command: { json, credential in
            let request = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
            XCTAssertEqual(request["action"] as? String, "set_policy")
            XCTAssertEqual(request["allowSubscriptionToMetered"] as? Bool, true)
            XCTAssertEqual((request["expectedRevision"] as? NSNumber)?.uint64Value, 4)
            XCTAssertNil(credential)
            return updatedJSON
        })
        await repository.refresh()
        XCTAssertFalse(repository.snapshot.providerPolicies?["openai"]?.allowSubscriptionToMetered ?? false)
        let succeeded = await repository.perform(.init(action: "set_policy", providerId: "openai", allowSubscriptionToMetered: true))
        XCTAssertTrue(succeeded)
        XCTAssertTrue(repository.snapshot.providerPolicies?["openai"]?.allowSubscriptionToMetered ?? false)
    }

    func testRoutingEvidenceDisplaysActualAccountAndRejectsOldIdentity() async throws {
        let repository = ProviderAccountRepository()
        let current = try snapshot(4)
        repository.configure(load: { current }, command: { _, _ in current })
        await repository.refresh()
        let account = try XCTUnwrap(repository.snapshot.accounts.first { $0.accountId == "b" })
        repository.recordRouting(#"{"status":"selected","selectedAccountId":"b","identityGeneration":0,"failedAccounts":[]}"#)
        XCTAssertFalse(repository.wasLastUsed(account))
        repository.recordRouting(#"{"status":"selected","selectedAccountId":"b","identityGeneration":1,"failedAccounts":[]}"#)
        XCTAssertTrue(repository.wasLastUsed(account))
        XCTAssertEqual(repository.accounts(providerId: "openai").map(\.accountId), ["a", "b"])
        repository.configure(load: { current }, command: { _, _ in current })
        XCTAssertFalse(repository.wasLastUsed(account))
    }
}
