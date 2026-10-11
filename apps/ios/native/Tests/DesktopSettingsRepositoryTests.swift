import XCTest
@testable import LingxiCode

@MainActor
final class DesktopSettingsRepositoryTests: XCTestCase {
    func testOwnLayerNeverCopiesMergedProviders() throws {
        let repository = DesktopSettingsRepository()
        repository.consume(.settingsSnapshot(effectiveJson: #"{"providers":{"user":{},"project":{}}}"#, provenanceJson: #"{"providers":"project"}"#, filesJson: nil, activeJson: nil, locked: [], layersJson: #"{"user":{"providers":{"user":{}}},"project":{"providers":{"project":{}}}}"#, mergedKeys: ["providers"]))
        let own = try XCTUnwrap(repository.ownValue(key: "providers", layer: .user) as? [String: Any])
        XCTAssertNotNil(own["user"])
        XCTAssertNil(own["project"])
        XCTAssertEqual(repository.provenanceLabel(for: "providers"), String(localized: "settings_parity_merged_layers"))
    }

    func testManagedAndDisconnectedWritesAreRejected() async {
        let repository = DesktopSettingsRepository()
        await repository.save(key: "fusion", json: "{}", layer: .managed)
        XCTAssertNotNil(repository.errorMessage)
        XCTAssertFalse(repository.saving)
        await repository.save(key: "fusion", json: "{}", layer: .user)
        XCTAssertNotNil(repository.errorMessage)
    }

    func testMcpHasIndependentScopeAndProviderPagesAreLayered() {
        XCTAssertFalse(DesktopSettingsEntry.all.first { $0.id == "mcp" }!.layered)
        XCTAssertTrue(DesktopSettingsEntry.all.first { $0.id == "custom-providers" }!.layered)
        XCTAssertTrue(DesktopSettingsEntry.all.first { $0.id == "fusion" }!.layered)
        XCTAssertEqual(Set(DesktopSettingsEntry.all.map(\.group)).count, 4)
        XCTAssertFalse(DesktopSettingsEntry.search("panelModels").contains { $0.id == "fusion" })
        XCTAssertFalse(DesktopSettingsEntry.search("").contains { $0.id == "fusion" })
        XCTAssertTrue(DesktopSettingsEntry.search("routing").contains { $0.id == "custom-providers" })
    }
    func testSaveNeedsMatchingEngineReadback() async {
        let repository = DesktopSettingsRepository()
        var submittedPatch: String?
        repository.configure { command in
            if case let .updateSettings(_, patchJson) = command { submittedPatch = patchJson }
        }
        repository.consume(snapshot(own: #"{"fusion":{"enabled":false}}"#))
        await repository.save(key: "fusion", json: #"{"enabled":true}"#, layer: .user)
        XCTAssertNotNil(submittedPatch)
        XCTAssertTrue(repository.saving, "Transport submission is not save confirmation")
        repository.consume(snapshot(own: #"{"fusion":{"enabled":true}}"#))
        XCTAssertFalse(repository.saving)
        XCTAssertNotNil(repository.statusMessage)
        XCTAssertNil(repository.errorMessage)
    }

    func testLockedKeyCannotSubmit() async {
        let repository = DesktopSettingsRepository()
        var mutations = 0
        repository.configure { command in if case .updateSettings = command { mutations += 1 } }
        repository.consume(.settingsSnapshot(effectiveJson: "{}", provenanceJson: "{}", filesJson: nil, activeJson: nil,
                                             locked: ["fusion"], layersJson: #"{"user":{}}"#, mergedKeys: []))
        await repository.save(key: "fusion", json: "{}", layer: .user)
        XCTAssertEqual(mutations, 0)
        XCTAssertNotNil(repository.errorMessage)
    }

    func testSourceChangeClearsPrivateSnapshotsAndDraftGeneration() {
        let repository = DesktopSettingsRepository()
        repository.consume(snapshot(own: #"{"providers":{"private":{}}}"#))
        let oldGeneration = repository.sourceGeneration
        repository.configure(submitter: nil)
        XCTAssertGreaterThan(repository.sourceGeneration, oldGeneration)
        XCTAssertTrue(repository.layers.isEmpty)
        XCTAssertFalse(repository.loaded)
    }

    func testMalformedLayerIsReadOnly() {
        let repository = DesktopSettingsRepository()
        repository.configure { _ in }
        repository.consume(.settingsSnapshot(effectiveJson: "{}", provenanceJson: "{}",
                                             filesJson: #"[{"layer":"user","exists":true,"parsed":false,"parse_error":"invalid JSON"}]"#,
                                             activeJson: nil, locked: [], layersJson: #"{"user":{}}"#, mergedKeys: []))
        XCTAssertFalse(repository.canEdit(key: "providers", layer: .user))
    }

    func testCredentialChangeWaitsForMatchingSecureStorageStatus() async {
        let repository = DesktopSettingsRepository()
        var operationID: UInt64?
        repository.configure { command in
            if case let .setProviderCredential(operationId, _, _) = command { operationID = operationId }
        }
        await repository.saveCredential(providerID: "test-profile", secret: "test-value")
        XCTAssertTrue(repository.saving)
        guard let operationID else { XCTFail("Credential command was not submitted"); return }
        repository.consume(.providerCredentialStatus(operationId: operationID, configuredProviderIds: ["test-profile"],
                                                     unavailableProviderIds: [], storageEncrypted: true, credentialPreviews: [:], error: nil))
        XCTAssertFalse(repository.saving)
        XCTAssertEqual(repository.credentialStates["test-profile"], true)
        XCTAssertEqual(repository.credentialStorageEncrypted, true)
    }

    func testPendingSaveCannotRefreshOrPublishErrorIntoNewSource() async {
        let repository = DesktopSettingsRepository()
        var blocked: CheckedContinuation<Void, Error>?
        repository.configure { command in
            if case .updateSettings = command {
                try await withCheckedThrowingContinuation { blocked = $0 }
            }
        }
        repository.consume(snapshot(own: "{}"))
        let task = Task { await repository.save(key: "fusion", json: "{}", layer: .user) }
        for _ in 0..<100 where blocked == nil { await Task.yield() }
        guard let blocked else { XCTFail("Save was not suspended"); task.cancel(); return }
        var refreshCount = 0
        repository.configure { command in if case .refreshListings = command { refreshCount += 1 } }
        await Task.yield()
        let expectedCount = refreshCount
        blocked.resume(throwing: NSError(domain: "old-source", code: 1))
        await task.value
        XCTAssertEqual(refreshCount, expectedCount)
        XCTAssertFalse(repository.loaded)
        XCTAssertFalse(repository.saving)
        XCTAssertFalse(repository.errorMessage?.contains("old-source") == true)
    }

    func testAdminResultMustMatchRequestedOperation() async {
        let repository = DesktopSettingsRepository()
        var operationID: UInt64?
        repository.configure { command in
            if case let .hookAdmin(command) = command { operationID = command.operationId }
        }
        await repository.admin(domain: "hook", action: "save_document", scope: "user", revision: "expected-sha",
                               payload: #"{"scope":"user","hooks":{}}"#, mutation: true)
        guard let operationID else { XCTFail("No correlated admin command"); return }
        repository.consume(.configurationOperation(domain: .hook, operationId: operationID + 1,
                                                     status: .succeeded, effect: .applied, message: "unrelated", detailsJson: nil))
        XCTAssertTrue(repository.saving)
        repository.consume(.configurationOperation(domain: .hook, operationId: operationID,
                                                     status: .failed, effect: .notApplicable, message: "revision conflict", detailsJson: nil))
        XCTAssertFalse(repository.saving)
        XCTAssertEqual(repository.errorMessage, "revision conflict")
    }

    func testSkillCatalogRetainsLoneSurrogateDistinctFromReplacementAndPairedText() throws {
        let json = #"{"entries":[{"id":"command:re\ud800view","name":"re\ud800view","source":"plugin","writable":false,"revision":null},{"id":"command:re\ufffdview","name":"re\ufffdview","source":"plugin","writable":false,"revision":null},{"id":"ordinary","name":"pair\ud83d\ude03","source":"bundled","writable":false,"revision":"sha"},{"id":"plain","name":"plain","source":"user","writable":true}],"trash":[{"trashId":"trash:\ud800","name":"old","source":"trash","revision":null}]}"#
        let catalog = try XCTUnwrap(SkillAdminCatalog.parse(json))

        let lone = try XCTUnwrap(catalog.entries.first { $0.name.codeUnits == [0x0072, 0x0065, 0xd800, 0x0076, 0x0069, 0x0065, 0x0077] })
        let replacement = try XCTUnwrap(catalog.entries.first { $0.identity.codeUnits == Array("command:re�view".utf16) })
        XCTAssertNotEqual(lone.identity, replacement.identity)
        XCTAssertEqual(lone.name.codeUnits, [0x0072, 0x0065, 0xd800, 0x0076, 0x0069, 0x0065, 0x0077])
        XCTAssertEqual(lone.name.displayValue, "re�view")
        XCTAssertNil(lone.revision)

        let paired = try XCTUnwrap(catalog.entries.first { $0.id == SkillAdminIdentity(codeUnits: Array("ordinary".utf16)).id })
        XCTAssertEqual(paired.name.codeUnits, Array("pair😃".utf16))
        XCTAssertEqual(paired.name.displayValue, "pair😃")
        XCTAssertEqual(catalog.entries.first { $0.id == SkillAdminIdentity(codeUnits: Array("plain".utf16)).id }?.name.displayValue, "plain")
        XCTAssertEqual(catalog.trash.first?.identity.codeUnits, Array("trash:".utf16) + [0xd800])
        XCTAssertNil(catalog.trash.first?.revision)
    }

    func testSkillCatalogUsesLastDuplicateIdentityAndRejectsMalformedJSON() throws {
        let duplicateKeyJSON = #"{"entries":[{"id":"discarded","id":"command:re\ud800view","name":"re\ud800view","source":"plugin","writable":false}],"trash":[]}"#
        let catalog = try XCTUnwrap(SkillAdminCatalog.parse(duplicateKeyJSON))
        XCTAssertEqual(catalog.entries.first?.identity.codeUnits, Array("command:re".utf16) + [0xd800] + Array("view".utf16))

        XCTAssertNil(SkillAdminCatalog.parse(#"{"entries":[{"id":"bad\x20","name":"bad","source":"user","writable":true}],"trash":[]}"#))
        XCTAssertNil(SkillAdminCatalog.parse(#"{"entries":[{"id":"bad","name":"bad","source":"user","writable":true},],"trash":[]}"#))
    }

    func testSkillDocumentCorrelationAndAdminSubmissionKeepExactIdentityUnits() async throws {
        let loneUnits = Array("command:re".utf16) + [0xd800] + Array("view".utf16)
        let replacementUnits = Array("command:re�view".utf16)
        let loneIdentity = SkillAdminIdentity(codeUnits: loneUnits)
        let replacementIdentity = SkillAdminIdentity(codeUnits: replacementUnits)
        let loneDocumentJSON = #"{"id":"command:re\ud800view","name":"re\ud800view","source":"plugin","markdown":"lone","writable":false,"revision":"r1"}"#
        let replacementDocumentJSON = #"{"id":"command:re\ufffdview","name":"re\ufffdview","source":"plugin","markdown":"replacement","writable":false,"revision":"r2"}"#
        let loneDocument = try XCTUnwrap(SkillAdminDocument.parse(loneDocumentJSON))
        let replacementDocument = try XCTUnwrap(SkillAdminDocument.parse(replacementDocumentJSON))

        XCTAssertTrue(loneDocument.matches(selectedIdentity: loneIdentity))
        XCTAssertFalse(replacementDocument.matches(selectedIdentity: loneIdentity))
        XCTAssertTrue(replacementDocument.matches(selectedIdentity: replacementIdentity))

        let repository = DesktopSettingsRepository()
        var submitted: [ClientCommand] = []
        repository.configure { command in submitted.append(command) }
        repository.consume(.skillCatalog(catalogJson: #"{"entries":[{"id":"command:re\ud800view","name":"re\ud800view","source":"plugin","writable":false}],"trash":[]}"#))
        repository.consume(.skillDocument(documentJson: loneDocumentJSON))
        XCTAssertEqual(repository.skillCatalog?.entries.first?.identity, loneIdentity)
        XCTAssertEqual(repository.skillDocument?.identity, loneIdentity)

        await repository.admin(domain: "skill", action: "get_document", skillTarget: loneIdentity)
        let command = try XCTUnwrap(submitted.compactMap { item -> SkillAdminCommandDto? in
            guard case let .skillAdmin(command) = item else { return nil }
            return command
        }.last)
        XCTAssertEqual(command.target?.codeUnits, loneUnits)
    }

    func testAdvancedProvidersRejectInlineSecretsBeforeSubmission() async {
        let repository = DesktopSettingsRepository()
        var mutations = 0
        repository.configure { command in if case .updateSettings = command { mutations += 1 } }
        repository.consume(snapshot(own: "{}"))
        await repository.save(key: "providers", json: #"{"custom":{"type":"openai","apiKey":"test-only","models":[{"id":"model"}]}}"#, layer: .user)
        XCTAssertEqual(mutations, 0)
        XCTAssertNotNil(repository.errorMessage)
        XCTAssertFalse(repository.errorMessage?.contains("test-only") == true)
    }

    func testSavedFileDifferencesRequireReconnectButManagedOverlayDoesNot() {
        let repository = DesktopSettingsRepository()
        repository.consume(.settingsSnapshot(effectiveJson: #"{"outputStyle":"Explanatory","managedOnly":true}"#,
                                             provenanceJson: #"{"outputStyle":"user","managedOnly":"managed"}"#,
                                             filesJson: nil, activeJson: #"{"outputStyle":"Concise"}"#,
                                             locked: ["managedOnly"], layersJson: #"{"user":{"outputStyle":"Explanatory"}}"#, mergedKeys: []))
        XCTAssertEqual(repository.pendingSettingsKeys, ["outputStyle"])
        XCTAssertTrue(repository.needsReconnect)
        repository.consume(.settingsSnapshot(effectiveJson: #"{"managedOnly":true}"#, provenanceJson: #"{"managedOnly":"managed"}"#,
                                             filesJson: nil, activeJson: "{}", locked: ["managedOnly"], layersJson: "{}", mergedKeys: []))
        XCTAssertFalse(repository.needsReconnect)
    }

    func testAdminRestartEffectSurvivesSnapshotUntilConnectionChanges() async {
        let repository = DesktopSettingsRepository()
        var operationID: UInt64?
        repository.configure { command in if case let .pluginAdmin(command) = command { operationID = command.operationId } }
        await repository.admin(domain: "plugin", action: "save_config", scope: "user", revision: "revision", payload: "{}", mutation: true)
        guard let operationID else { XCTFail("No admin command"); return }
        repository.consume(.configurationOperation(domain: .plugin, operationId: operationID, status: .succeeded,
                                                     effect: .restartRequired, message: "saved", detailsJson: nil))
        repository.consume(.settingsSnapshot(effectiveJson: "{}", provenanceJson: "{}", filesJson: nil, activeJson: "{}",
                                             locked: [], layersJson: "{}", mergedKeys: []))
        XCTAssertTrue(repository.needsReconnect)
        repository.configure(submitter: nil)
        XCTAssertFalse(repository.needsReconnect)
    }

    private func snapshot(own: String) -> ClientEvent {
        .settingsSnapshot(effectiveJson: own, provenanceJson: "{}", filesJson: nil, activeJson: nil,
                          locked: [], layersJson: "{\"user\":\(own)}", mergedKeys: [])
    }

}
