import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class ProviderRepositoryTests: XCTestCase {
    private var tempDirectory: URL!
    private var persistenceURL: URL!

    override func setUpWithError() throws {
        tempDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("provider-repo-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDirectory, withIntermediateDirectories: true)
        persistenceURL = tempDirectory.appendingPathComponent("provider-settings.json")
    }

    override func tearDownWithError() throws {
        if let tempDirectory {
            try? FileManager.default.removeItem(at: tempDirectory)
        }
    }

    private func runtimeModelDetails(
        reference: String,
        providerID: String,
        providerLabel: String,
        displayName: String,
        modelID: String
    ) -> ModelRuntimeDetails {
        ModelRuntimeDetails(
            reference: reference,
            providerId: providerID,
            providerLabel: providerLabel,
            displayName: displayName,
            modelId: modelID,
            description: nil,
            family: nil,
            status: nil,
            releaseDate: nil,
            lastUpdated: nil,
            knowledgeCutoff: nil,
            inputModalities: [],
            outputModalities: [],
            contextWindowTokens: nil,
            maxInputTokens: nil,
            maxOutputTokens: nil,
            openWeights: nil,
            attachments: nil,
            temperatureControl: nil,
            pricing: nil,
            capabilities: ModelCapabilitiesDto(
                streaming: true,
                tools: true,
                vision: false,
                documents: false,
                reasoning: false,
                structuredOutput: false
            ),
            reasoning: ReasoningControlSpecDto(
                options: [],
                budgetRange: nil,
                providerDefault: .automatic,
                forcedReasoning: false,
                editable: false,
                disabledReason: nil
            )
        )
    }

    func testLaunchSnapshotUsesBuiltInProfileForOfficialEndpoint() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")
        let openAI = repository.addProfile(presetID: "openai")

        repository.updateProfile(anthropic) {
            $0.modelID = "claude-opus-4"
            $0.baseURL = "https://api.anthropic.com"
            $0.enabled = false
        }
        repository.updateProfile(openAI) {
            $0.modelID = "gpt-4o"
            $0.baseURL = "https://api.openai.com/v1"
        }
        repository.setDefaultProfile(openAI)

        let snapshot = repository.makeLaunchSnapshot()

        XCTAssertEqual(snapshot.defaultModelID, "openai/gpt-4o")
        XCTAssertEqual(snapshot.enabledProfileIDs, ["openai"])
        XCTAssertEqual(try jsonObject(from: snapshot.providerProfilesJSON)?.count, 0)
        XCTAssertTrue(snapshot.routingJSON.contains("\"mobileEnabledProfiles\":[\"openai\"]"))
        XCTAssertTrue(snapshot.routingJSON.contains("\"retry\":{\"backoffMs\":500,\"maxAttempts\":10}"))
    }

    func testChatGPTOAuthHasItsOwnBuiltInProfileAndNoAPIKeyEnvironmentBinding() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let chatGPT = repository.addProfile(presetID: "openai-chatgpt")

        let profile = try XCTUnwrap(repository.state(for: chatGPT)?.profile)
        XCTAssertEqual(repository.oauthProvider(for: profile.presetID), "openai-chatgpt")
        XCTAssertEqual(profile.baseURL, "https://chatgpt.com/backend-api/codex")

        let snapshot = repository.makeLaunchSnapshot()
        XCTAssertEqual(snapshot.defaultModelID, "openai-chatgpt/gpt-5.6-sol")
        XCTAssertEqual(snapshot.enabledProfileIDs, ["openai-chatgpt"])
        XCTAssertEqual(try jsonObject(from: snapshot.providerProfilesJSON)?.count, 0)
    }

    func testVisionDelegationDefaultsEnabledAndPersists() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)

        XCTAssertTrue(repository.makeLaunchSnapshot().visionDelegationEnabled)

        repository.setVisionDelegationEnabled(false)

        XCTAssertFalse(repository.makeLaunchSnapshot().visionDelegationEnabled)
        let reloaded = ProviderRepository(persistenceURL: persistenceURL)
        XCTAssertFalse(reloaded.makeLaunchSnapshot().visionDelegationEnabled)
    }

    func testStoredProfileDecodesMissingVisibilityFieldsToShowAll() throws {
        let data = try XCTUnwrap(
            """
            {"id":"openai","presetID":"openai","name":"OpenAI","baseURL":"https://api.openai.com/v1","modelID":"gpt-5.6-sol","enabled":true,"isDefault":false}
            """.data(using: .utf8)
        )

        let profile = try JSONDecoder().decode(ProviderStoredProfile.self, from: data)

        XCTAssertTrue(profile.showInModelPicker)
        XCTAssertNil(profile.visibleModelIDs)
    }

    func testStoredProfileEncodesVisibleModelIdsUsingCrossPlatformKey() throws {
        let profile = ProviderStoredProfile(
            id: "openai",
            presetID: "openai",
            name: "OpenAI",
            baseURL: "https://api.openai.com/v1",
            modelID: "gpt-5.6-sol",
            enabled: true,
            isDefault: false,
            showInModelPicker: true,
            visibleModelIDs: ["gpt-5.6-sol"]
        )

        let data = try JSONEncoder().encode(profile)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

        XCTAssertEqual(object["visibleModelIds"] as? [String], ["gpt-5.6-sol"])
        XCTAssertNil(object["visibleModelIDs"])
    }

    func testVisibleModelHelpersHonorProviderSwitchAndExplicitAllowlist() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openai = repository.addProfile(presetID: "openai")
        let deepseek = repository.addProfile(presetID: "deepseek")
        repository.updateProfile(openai) {
            $0.visibleModelIDs = ["gpt-5.6-sol"]
        }
        repository.updateProfile(deepseek) {
            $0.showInModelPicker = false
        }

        let openaiProfile = try XCTUnwrap(repository.state(for: openai)?.profile)
        let deepseekProfile = try XCTUnwrap(repository.state(for: deepseek)?.profile)

        XCTAssertEqual(
            repository.visibleModelIDs(
                for: openaiProfile,
                from: ["gpt-5.6-sol", "gpt-5.7-preview", "gpt-5.6-sol"]
            ),
            ["gpt-5.6-sol"]
        )
        XCTAssertEqual(
            repository.visibleModelIDs(
                for: deepseekProfile,
                from: ["deepseek-flash"]
            ),
            ["deepseek-flash"]
        )
        XCTAssertEqual(
            repository.visibleModelReferences([
                "openai/gpt-5.6-sol",
                "openai/gpt-5.7-preview",
                "deepseek/deepseek-flash",
                "community/custom-model",
            ]),
            ["openai/gpt-5.6-sol", "community/custom-model"]
        )
    }

    func testAnthropicOAuthCountsAsCredentialWithoutReplacingAPIKeyState() throws {
        let profile = ProviderStoredProfile(
            id: "anthropic",
            presetID: "anthropic",
            name: "Anthropic",
            baseURL: "https://api.anthropic.com",
            modelID: "claude-sonnet-4",
            enabled: true,
            isDefault: true
        )
        var state = ProviderProfileState(profile: profile, credentialState: .configured)
        state.oauthState = ProviderOAuthState(
            provider: "anthropic",
            signedIn: true,
            accountLabel: "account@example.test",
            accountID: nil,
            organizationID: "org-test",
            fedramp: false
        )

        XCTAssertTrue(state.hasStoredAPIKey)
        XCTAssertTrue(state.hasStoredCredential)
        XCTAssertEqual(state.maskedCredentialSummary, String(localized: "settings_provider_credential_api_key_oauth"))
        XCTAssertEqual(state.credentialFieldMask, "••••••••••••")
    }

    func testCredentialStatusRefreshIncludesAnthropicAPIKeyAlongsideOAuth() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        _ = repository.addProfile(presetID: "anthropic")
        _ = repository.addProfile(presetID: "openai-chatgpt")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        await repository.refreshCredentialStatus()

        guard case let .listProviderCredentials(_, providerIDs, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected list provider credentials")
        }
        XCTAssertEqual(providerIDs, ["anthropic"], "Anthropic API Key must be restored independently of OAuth")
    }

    func testOAuthProfileEndpointCannotBecomeASeparateUserProfile() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")
        let chatGPT = repository.addProfile(presetID: "openai-chatgpt")

        repository.updateProfile(anthropic) {
            $0.baseURL = "http://untrusted.example.test"
        }
        repository.updateProfile(chatGPT) {
            $0.baseURL = "https://proxy.example.test/v1"
        }

        XCTAssertEqual(repository.state(for: anthropic)?.profile.baseURL, "https://api.anthropic.com")
        XCTAssertEqual(repository.state(for: chatGPT)?.profile.baseURL, "https://chatgpt.com/backend-api/codex")
        XCTAssertEqual(try jsonObject(from: repository.makeLaunchSnapshot().providerProfilesJSON)?.count, 0)
    }

    func testLaunchSnapshotCreatesIndependentProfileForCustomOfficialPresetEndpoint() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.updateProfile(openAI) {
            $0.baseURL = "https://proxy.example.com/v1"
            $0.modelID = "gpt-4o"
        }

        let snapshot = repository.makeLaunchSnapshot()
        let profiles = try XCTUnwrap(jsonObject(from: snapshot.providerProfilesJSON))
        let customProfile = try XCTUnwrap(profiles["openai-user"] as? [String: Any])

        XCTAssertEqual(snapshot.defaultModelID, "openai-user/gpt-4o")
        XCTAssertEqual(snapshot.enabledProfileIDs, ["openai-user"])
        XCTAssertEqual(customProfile["type"] as? String, "openai-responses")
        XCTAssertEqual(customProfile["baseUrl"] as? String, "https://proxy.example.com/v1")
        XCTAssertEqual(customProfile["apiKeyEnv"] as? String, "OPENAI_API_KEY")
        XCTAssertEqual(
            (customProfile["models"] as? [[String: String]])?.map { $0["id"] },
            ["gpt-4o", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]
        )
    }

    func testApplyValidationRequiresCredentialForEnabledProfile() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")

        await repository.applyChanges(openAI)

        let state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.validationMessage, ProviderProfileValidationError.missingCredential.errorDescription)
    }

    func testPersistenceExcludesPendingSecrets() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.updateProfile(openAI) {
            $0.baseURL = "https://proxy.example.com/v1"
            $0.modelID = "gpt-4o-mini"
        }
        repository.setRetryMaxAttempts(5)
        repository.setRetryBackoffMs(900)
        repository.stageSecret("sk-secret-should-not-persist", for: openAI)

        let data = try Data(contentsOf: persistenceURL)
        let string = try XCTUnwrap(String(data: data, encoding: .utf8))

        XCTAssertTrue(string.contains("https://proxy.example.com/v1"))
        XCTAssertTrue(string.contains("gpt-4o-mini"))
        XCTAssertTrue(string.contains("\"retryMaxAttempts\":5"))
        XCTAssertTrue(string.contains("\"retryBackoffMs\":900"))
        XCTAssertFalse(string.contains("sk-secret-should-not-persist"))
    }

    func testLegacyPersistenceWithoutRoutingLoadsDefaults() throws {
        let legacyJSON = """
        {"version":1,"profiles":[{"id":"openai","presetID":"openai","name":"OpenAI","baseURL":"https://api.openai.com/v1","modelID":"gpt-4o","enabled":true,"isDefault":true}]}
        """
        try legacyJSON.data(using: .utf8)?.write(to: persistenceURL)

        let repository = ProviderRepository(persistenceURL: persistenceURL)

        XCTAssertEqual(repository.routingSettings, ProviderRoutingSettings(retryMaxAttempts: 10, retryBackoffMs: 500, fallbackProfileIDs: []))
        XCTAssertEqual(repository.state(for: "openai")?.profile.modelID, "gpt-4o")
    }

    func testDeepSeekPresetMatchesAndroidOfficialModels() throws {
        let preset = try XCTUnwrap(Presets.llm.first(where: { $0.id == "deepseek" }))

        XCTAssertEqual(preset.defaultUrl, "https://api.deepseek.com")
        XCTAssertEqual(preset.models, ["deepseek-flash", "deepseek-v4-pro"])

        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        let profile = try XCTUnwrap(repository.state(for: deepSeek)?.profile)
        XCTAssertEqual(profile.baseURL, "https://api.deepseek.com")
        XCTAssertEqual(profile.modelID, "deepseek-flash")
    }

    func testDeepSeekOfficialEndpointDoesNotDuplicateBuiltInEngineProfile() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        repository.setDefaultProfile(deepSeek)

        let snapshot = repository.makeLaunchSnapshot()

        XCTAssertEqual(snapshot.defaultModelID, "deepseek/deepseek-flash")
        XCTAssertEqual(snapshot.enabledProfileIDs, ["deepseek"])
        XCTAssertEqual(try jsonObject(from: snapshot.providerProfilesJSON)?.count, 0)
    }

    func testCredentialCommandsUseEngineProfileIDAndUpdateSettingsRow() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.updateProfile(openAI) {
            $0.baseURL = "https://proxy.example.com/v1"
        }
        repository.stageSecret("sk-proxy", for: openAI)
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        let apply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)
        guard case let .setProviderCredential(operationId, providerId, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        XCTAssertEqual(providerId, "openai-user")

        repository.handle(event: .providerCredentialStatus(
            operationId: operationId,
            configuredProviderIds: ["openai-user"],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await apply.value

        XCTAssertEqual(repository.state(for: openAI)?.credentialState, .configured)
        XCTAssertEqual(repository.state(for: openAI)?.pendingSecret, "")
    }

    func testStoredCredentialUsesMaskAndOnlyDraftCredentialCanBeRevealed() throws {
        let profile = ProviderStoredProfile(
            id: "deepseek",
            presetID: "deepseek",
            name: "DeepSeek",
            baseURL: "https://api.deepseek.com",
            modelID: "deepseek-flash",
            enabled: true,
            isDefault: true
        )
        var state = ProviderProfileState(
            profile: profile,
            credentialState: .configured
        )

        XCTAssertEqual(state.credentialFieldMask, "••••••••••••")
        XCTAssertFalse(state.canRevealCredential)

        state.pendingSecret = "sk-replacement"

        XCTAssertNil(state.credentialFieldMask)
        XCTAssertTrue(state.canRevealCredential)
    }

    func testDiscardCredentialChangesKeepsStoredCredential() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(operationId, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected credential status request")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: operationId,
            configuredProviderIds: [deepSeek],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))

        repository.stageSecret("sk-replacement", for: deepSeek)
        repository.discardCredentialChanges(for: deepSeek)

        var state = try XCTUnwrap(repository.state(for: deepSeek))
        XCTAssertEqual(state.pendingSecret, "")
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertEqual(state.credentialFieldMask, "••••••••••••")

        repository.clearCredentialRequest(for: deepSeek)

        repository.discardCredentialChanges(for: deepSeek)

        state = try XCTUnwrap(repository.state(for: deepSeek))
        XCTAssertEqual(state.pendingSecret, "")
        XCTAssertFalse(state.clearCredentialOnApply)
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertEqual(state.connectionState, .idle)
    }

    func testLegacyDeepSeekProfileMigratesToCurrentEndpointAndModel() throws {
        let legacyJSON = """
        {"version":2,"profiles":[{"id":"deepseek","presetID":"deepseek","name":"DeepSeek","baseURL":"https://api.deepseek.com/v1/","modelID":"deepseek-reasoner","enabled":true,"isDefault":true}],"routing":{"retryMaxAttempts":10,"retryBackoffMs":500,"fallbackProfileIDs":[]}}
        """
        try XCTUnwrap(legacyJSON.data(using: .utf8)).write(to: persistenceURL)

        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let profile = try XCTUnwrap(repository.state(for: "deepseek")?.profile)

        XCTAssertEqual(profile.baseURL, "https://api.deepseek.com")
        XCTAssertEqual(profile.modelID, "deepseek-flash")

        let persisted = try String(contentsOf: persistenceURL, encoding: .utf8)
        XCTAssertFalse(persisted.contains("api.deepseek.com/v1"))
        XCTAssertFalse(persisted.contains("deepseek-reasoner"))
    }

    func testRoutingJsonIncludesRetryAndOrderedFallbackForDefaultModel() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        let deepseek = repository.addProfile(presetID: "deepseek")

        repository.setDefaultProfile(openAI)
        repository.updateProfile(kimi) {
            $0.modelID = "kimi-k3"
            $0.enabled = true
        }
        repository.updateProfile(deepseek) {
            $0.modelID = "deepseek-flash"
            $0.enabled = true
        }
        repository.setRetryMaxAttempts(4)
        repository.setRetryBackoffMs(1200)
        repository.toggleFallbackProfile(kimi)
        repository.toggleFallbackProfile(deepseek)
        repository.moveFallbackProfile(deepseek, by: -1)

        let snapshot = repository.makeLaunchSnapshot()
        let routing = try XCTUnwrap(jsonObject(from: snapshot.routingJSON))
        let retry = try XCTUnwrap(routing["retry"] as? [String: Any])
        let fallback = try XCTUnwrap(routing["fallback"] as? [String: Any])
        let targets = try XCTUnwrap(fallback["openai/gpt-5.6-sol"] as? [String])

        XCTAssertEqual(retry["maxAttempts"] as? Int, 4)
        XCTAssertEqual(retry["backoffMs"] as? Int, 1200)
        XCTAssertEqual(targets, ["deepseek/deepseek-flash", "kimi/kimi-k3"])
    }

    func testNewDraftIsNotPublishedUntilApply() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        var draft = repository.makeNewDraft(presetID: "openai")
        draft.profile.name = "Unpublished OpenAI"

        XCTAssertNil(repository.state(for: draft.id))
        XCTAssertFalse(repository.makeLaunchSnapshot().profiles.contains { $0.settingsID == draft.id })

        let applied = await repository.applyDraft(draft)
        XCTAssertFalse(applied, "an enabled draft without a credential must fail validation")
        XCTAssertNil(repository.state(for: draft.id), "failed draft validation must not create a profile")
    }

    func testNewDraftUsesAConversationalDefaultModelWhenCatalogStartsWithNonChatModel() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)

        let draft = repository.makeNewDraft(presetID: "openai")

        XCTAssertEqual(draft.profile.modelID, "gpt-5.6-sol")
    }

    func testOAuthDraftConnectionUsesOAuthTesterInsteadOfAPIKeyTester() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai-chatgpt")
        var apiKeyTesterCalled = false
        var oauthTesterCalled = false
        repository.configure(
            submitCommand: nil,
            testConnection: { _, _ in
                apiKeyTesterCalled = true
                return .success()
            },
            testOAuthConnection: { provider, profile in
                oauthTesterCalled = true
                XCTAssertEqual(provider, "openai-chatgpt")
                XCTAssertEqual(profile.id, "openai-chatgpt")
                return .success(message: "OAuth probe")
            }
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.oauthState = ProviderOAuthState(
            provider: "openai-chatgpt",
            signedIn: true,
            accountLabel: "test",
            accountID: nil,
            organizationID: nil,
            fedramp: false
        )
        let tested = await repository.testConnection(for: draft)

        XCTAssertTrue(oauthTesterCalled)
        XCTAssertFalse(apiKeyTesterCalled)
        XCTAssertEqual(tested.connectionState, .connected)
        XCTAssertEqual(tested.detailMessage, "OAuth probe")
    }

    func testApplyDraftKeepsSavedProfileWhenReconnectFailsAfterPersistence() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        var draft = repository.makeNewDraft(presetID: "openai")
        draft.profile.name = "Saved before reconnect"
        draft.profile.enabled = false
        repository.configure(
            submitCommand: nil,
            applyReconnect: { _ in
                throw NSError(domain: "ProviderRepositoryTests", code: 1)
            }
        )

        let applied = await repository.applyDraft(draft)
        XCTAssertFalse(applied)
        XCTAssertEqual(repository.state(for: draft.id)?.profile.name, "Saved before reconnect")
        XCTAssertEqual(repository.state(for: draft.id)?.connectionState, .failed)

        let reloaded = ProviderRepository(persistenceURL: persistenceURL)
        XCTAssertEqual(reloaded.state(for: draft.id)?.profile.name, "Saved before reconnect")
    }

    func testApplyDraftVisibilityToggleOnlySkipsReconnectAndKeepsLaunchSnapshot() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "deepseek")
        repository.updateProfile(id) {
            $0.enabled = false
            $0.isDefault = false
        }
        let before = repository.makeLaunchSnapshot()
        var reconnectSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: nil,
            applyReconnect: { snapshot in
                reconnectSnapshots.append(snapshot)
            }
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.profile.showInModelPicker = false

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertTrue(reconnectSnapshots.isEmpty)
        XCTAssertEqual(repository.makeLaunchSnapshot(), before)
        XCTAssertFalse(try XCTUnwrap(repository.state(for: id)).profile.showInModelPicker)

        let reloaded = ProviderRepository(persistenceURL: persistenceURL)
        XCTAssertFalse(try XCTUnwrap(reloaded.state(for: id)).profile.showInModelPicker)
    }

    func testApplyDraftVisibleModelAllowlistOnlySkipsReconnectAndKeepsLaunchSnapshot() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        repository.updateProfile(id) {
            $0.enabled = false
            $0.isDefault = false
        }
        let before = repository.makeLaunchSnapshot()
        var reconnectSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: nil,
            applyReconnect: { snapshot in
                reconnectSnapshots.append(snapshot)
            }
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.profile.visibleModelIDs = ["gpt-5.6-sol"]

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertTrue(reconnectSnapshots.isEmpty)
        XCTAssertEqual(repository.makeLaunchSnapshot(), before)
        XCTAssertEqual(try XCTUnwrap(repository.state(for: id)).profile.visibleModelIDs, ["gpt-5.6-sol"])

        let reloaded = ProviderRepository(persistenceURL: persistenceURL)
        XCTAssertEqual(try XCTUnwrap(reloaded.state(for: id)).profile.visibleModelIDs, ["gpt-5.6-sol"])
    }

    func testApplyDraftMasterTogglePreservesExplicitModelSelectionWhenTurnedBackOn() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        repository.updateProfile(id) {
            $0.enabled = false
            $0.isDefault = false
            $0.visibleModelIDs = ["gpt-5.6-terra"]
            $0.showInModelPicker = false
        }
        var reconnectSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: nil,
            applyReconnect: { snapshot in
                reconnectSnapshots.append(snapshot)
            }
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        XCTAssertEqual(draft.profile.visibleModelIDs, ["gpt-5.6-terra"])
        XCTAssertFalse(draft.profile.showInModelPicker)
        draft.profile.showInModelPicker = true

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        let stored = try XCTUnwrap(repository.state(for: id)?.profile)
        XCTAssertTrue(stored.showInModelPicker)
        XCTAssertEqual(stored.visibleModelIDs, ["gpt-5.6-terra"])
        XCTAssertTrue(reconnectSnapshots.isEmpty)
    }

    func testApplyDraftRuntimeChangeStillReconnectsWhenVisibilityAlsoChanges() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        repository.updateProfile(id) {
            $0.enabled = false
            $0.isDefault = false
        }
        let before = repository.makeLaunchSnapshot()
        var reconnectSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: nil,
            applyReconnect: { snapshot in
                reconnectSnapshots.append(snapshot)
            }
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.profile.modelID = "gpt-5.6-terra"
        draft.profile.visibleModelIDs = ["gpt-5.6-terra"]

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertEqual(reconnectSnapshots.count, 1)
        XCTAssertNotEqual(reconnectSnapshots[0], before)
        XCTAssertEqual(reconnectSnapshots[0], repository.makeLaunchSnapshot())
    }

    func testApplyDraftCredentialOnlyStillReconnectsWhenLaunchSnapshotIsUnchanged() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        repository.updateProfile(id) {
            $0.enabled = false
            $0.isDefault = false
        }
        let before = repository.makeLaunchSnapshot()
        var reconnectSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: { command in
                try await recorder.submit(command: command)
            },
            applyReconnect: { snapshot in
                reconnectSnapshots.append(snapshot)
            }
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.pendingSecret = "sk-openai"

        let apply = Task { await repository.applyDraft(draft) }
        await waitForCommandCount(1, recorder: recorder)
        guard case let .setProviderCredential(operationId, providerId, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        XCTAssertEqual(providerId, id)

        repository.handle(event: .providerCredentialStatus(
            operationId: operationId,
            configuredProviderIds: [id],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))

        let applied = await apply.value
        XCTAssertTrue(applied)
        XCTAssertEqual(repository.makeLaunchSnapshot(), before)
        XCTAssertEqual(reconnectSnapshots.count, 1)
        XCTAssertEqual(reconnectSnapshots[0], before)
    }

    func testEditingAndDiscardingDraftDoesNotChangeSavedSnapshot() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "deepseek")
        let before = repository.makeLaunchSnapshot()
        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.profile.name = "Temporary name"
        draft.profile.modelID = "temporary-model"
        draft.profile.baseURL = "https://proxy.example.test/v1"
        repository.discardDraft(draft)

        XCTAssertEqual(repository.makeLaunchSnapshot(), before)
        XCTAssertEqual(repository.state(for: id)?.profile.name, "DeepSeek")
    }

    func testSummaryCombinesSavedConfigurationWithRuntimeState() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let first = repository.addProfile(presetID: "deepseek")
        _ = repository.addProfile(presetID: "openai")
        repository.setDefaultProfile(first)
        repository.updateRuntimeSnapshot(
            models: ["deepseek/deepseek-flash"],
            activeModelID: "openai/gpt-4o",
            activeProfileID: "openai",
            error: "runtime fallback"
        )

        let summary = repository.settingsSummary
        XCTAssertEqual(summary.defaultProfile?.id, first)
        XCTAssertEqual(summary.defaultModelID, "deepseek-flash")
        XCTAssertEqual(summary.enabledCount, 2)
        XCTAssertEqual(summary.runtime.activeModelID, "openai/gpt-4o")
        XCTAssertEqual(summary.runtime.lastError, "runtime fallback")
        XCTAssertFalse(summary.runtimeMatchesDefault)

        repository.setDefaultProfile("openai")
        repository.updateRuntimeSnapshot(
            models: ["openai/gpt-5.6-sol"],
            activeModelID: "openai/gpt-5.6-sol",
            activeProfileID: "openai"
        )
        XCTAssertTrue(repository.settingsSummary.runtimeMatchesDefault)
    }

    func testCatalogRefreshKeepsOnlyKnownBuiltInPresetsInAddPicker() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let entry = ProviderCatalogEntry(
            id: "engine-only",
            displayName: "Engine Only",
            baseURL: "https://engine.example.test/v1",
            protocolName: "OpenAiChat",
            authName: "ApiKey",
            credentialEnv: "ENGINE_ONLY_KEY",
            models: ["engine-model"],
            modelDetails: [:]
        )
        repository.configure(submitCommand: nil, providerCatalog: { [entry] })

        await repository.refreshCatalog()

        XCTAssertEqual(repository.catalogPresets.map(\.id), ["custom"])
        XCTAssertEqual(repository.preset(for: "engine-only").models, ["engine-model"])
        XCTAssertEqual(repository.preset(for: "engine-only").defaultUrl, "https://engine.example.test/v1")
    }

    func testEditorModelCatalogUsesLoadingStateUntilBuiltInCatalogArrives() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        let profile = try XCTUnwrap(repository.state(for: id)?.profile)
        repository.configure(submitCommand: nil, providerCatalog: { [] })

        if case .loading = repository.editorModelCatalog(
            for: profile,
            runtimeModelReferences: []
        ) {
        } else {
            XCTFail("expected built-in editor catalog to stay in loading state while shared catalog is empty")
        }
    }

    func testEditorModelCatalogShowsUnavailableAfterLoadedCatalogOmitsBuiltInProvider() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        let profile = try XCTUnwrap(repository.state(for: id)?.profile)
        repository.configure(submitCommand: nil, providerCatalog: { [] })

        let refreshed = await repository.refreshCatalog()
        XCTAssertTrue(refreshed)

        if case .unavailable = repository.editorModelCatalog(
            for: profile,
            runtimeModelReferences: []
        ) {
        } else {
            XCTFail("expected built-in editor catalog to become unavailable after a loaded empty catalog")
        }
    }

    func testEditorModelCatalogDoesNotShowStaleBuiltInCurrentModelOutsideSharedCatalog() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let id = repository.addProfile(presetID: "openai")
        repository.updateProfile(id) {
            $0.modelID = "gpt-stale-hidden"
        }
        let profile = try XCTUnwrap(repository.state(for: id)?.profile)
        let entry = ProviderCatalogEntry(
            id: "openai",
            displayName: "OpenAI",
            baseURL: "https://api.openai.com/v1",
            protocolName: "ChatGPT API",
            authName: "ApiKey",
            credentialEnv: "OPENAI_API_KEY",
            models: ["gpt-5.6-sol", "gpt-5.6-terra"],
            modelDetails: [:]
        )
        repository.configure(submitCommand: nil, providerCatalog: { [entry] })
        let refreshed = await repository.refreshCatalog()
        XCTAssertTrue(refreshed)

        guard case let .ready(models, _) = repository.editorModelCatalog(
            for: profile,
            runtimeModelReferences: ["openai/gpt-stale-hidden"]
        ) else {
            return XCTFail("expected built-in editor catalog to be ready")
        }

        XCTAssertEqual(models, ["gpt-5.6-sol", "gpt-5.6-terra"])
        XCTAssertFalse(models.contains("gpt-stale-hidden"))
    }

    func testVisibleModelReferencesUsesAuthoritativeCatalogAfterLoad() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        _ = repository.addProfile(presetID: "openai")
        let entry = ProviderCatalogEntry(
            id: "openai",
            displayName: "OpenAI",
            baseURL: "https://api.openai.com/v1",
            protocolName: "ChatGPT API",
            authName: "ApiKey",
            credentialEnv: "OPENAI_API_KEY",
            models: ["gpt-5.6-sol", "gpt-5.6-terra"],
            modelDetails: [
                "openai/gpt-5.6-sol": runtimeModelDetails(
                    reference: "openai/gpt-5.6-sol",
                    providerID: "openai",
                    providerLabel: "OpenAI",
                    displayName: "GPT-5.6 Sol",
                    modelID: "gpt-5.6-sol"
                ),
                "openai/gpt-5.6-terra": runtimeModelDetails(
                    reference: "openai/gpt-5.6-terra",
                    providerID: "openai",
                    providerLabel: "OpenAI",
                    displayName: "GPT-5.6 Terra",
                    modelID: "gpt-5.6-terra"
                ),
            ]
        )
        repository.configure(submitCommand: nil, providerCatalog: { [entry] })

        XCTAssertEqual(
            repository.visibleModelReferences([
                "openai/gpt-5.6-sol",
                "openai/gpt-stale-hidden",
            ]),
            ["openai/gpt-5.6-sol", "openai/gpt-stale-hidden"],
            "before catalog arrival, preserve engine rows"
        )

        let refreshed = await repository.refreshCatalog()
        XCTAssertTrue(refreshed)

        XCTAssertEqual(
            repository.visibleModelReferences([
                "openai/gpt-5.6-sol",
                "openai/gpt-5.6-terra",
                "openai/gpt-stale-hidden",
            ]),
            ["openai/gpt-5.6-sol", "openai/gpt-5.6-terra"]
        )
    }

    func testVisibleModelReferencesDefaultsToVisibleForCatalogMemberWithoutStoredProfile() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let entry = ProviderCatalogEntry(
            id: "openai",
            displayName: "OpenAI",
            baseURL: "https://api.openai.com/v1",
            protocolName: "ChatGPT API",
            authName: "ApiKey",
            credentialEnv: "OPENAI_API_KEY",
            models: ["gpt-5.6-sol"],
            modelDetails: [
                "openai/gpt-5.6-sol": runtimeModelDetails(
                    reference: "openai/gpt-5.6-sol",
                    providerID: "openai",
                    providerLabel: "OpenAI",
                    displayName: "GPT-5.6 Sol",
                    modelID: "gpt-5.6-sol"
                ),
            ]
        )
        repository.configure(submitCommand: nil, providerCatalog: { [entry] })
        let refreshed = await repository.refreshCatalog()
        XCTAssertTrue(refreshed)

        XCTAssertEqual(
            repository.visibleModelReferences([
                "openai/gpt-5.6-sol",
            ]),
            ["openai/gpt-5.6-sol"]
        )
    }

    func testRoutingFiltersInvalidFallbackSelections() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        let deepseek = repository.addProfile(presetID: "deepseek")

        repository.setDefaultProfile(openAI)
        repository.updateProfile(kimi) { $0.modelID = "kimi-k3" }
        repository.updateProfile(deepseek) {
            $0.modelID = ""
            $0.enabled = false
        }

        repository.toggleFallbackProfile(kimi)
        repository.toggleFallbackProfile(openAI)
        repository.toggleFallbackProfile(deepseek)

        let candidates = repository.fallbackCandidates()
        XCTAssertEqual(repository.routingSettings.fallbackProfileIDs, ["kimi"])
        XCTAssertEqual(candidates.map(\.profileID), ["kimi"])
        let snapshot = repository.makeLaunchSnapshot()
        let routing = try XCTUnwrap(jsonObject(from: snapshot.routingJSON))
        let fallback = try XCTUnwrap(routing["fallback"] as? [String: Any])
        let targets = try XCTUnwrap(fallback["openai/gpt-5.6-sol"] as? [String])
        XCTAssertEqual(targets, ["kimi/kimi-k3"])
        XCTAssertFalse(targets.contains(where: { $0.contains("openai/") || $0.contains("deepseek/") }))
    }

    func testApplyRoutingChangesInvokesReconnectWithValidatedRoutingSnapshot() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.setDefaultProfile(openAI)
        repository.updateProfile(kimi) { $0.modelID = "kimi-k3" }
        repository.toggleFallbackProfile(kimi)

        var appliedSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: nil,
            applyReconnect: { snapshot in
                appliedSnapshots.append(snapshot)
            }
        )

        await repository.applyRoutingChanges(
            retryMaxAttemptsText: "6",
            retryBackoffMsText: "1500"
        )

        XCTAssertEqual(appliedSnapshots.count, 1)
        let routing = try XCTUnwrap(jsonObject(from: appliedSnapshots[0].routingJSON))
        let retry = try XCTUnwrap(routing["retry"] as? [String: Any])
        XCTAssertEqual(retry["maxAttempts"] as? Int, 6)
        XCTAssertEqual(retry["backoffMs"] as? Int, 1500)
        XCTAssertFalse(repository.routingDirty)
        XCTAssertEqual(repository.routingMessage, "已应用路由并请求重连。")
    }

    func testListStatusCorrelationPreservesUnavailableConfiguredState() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        repository.stageSecret("sk-openai", for: openAI)
        let apply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)

        guard case let .setProviderCredential(operationId: setOperationID, providerId, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        XCTAssertEqual(providerId, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: setOperationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await apply.value

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(listOperationID, providerIds, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected list provider credentials")
        }
        XCTAssertEqual(Set(providerIds), Set([openAI, kimi]))

        repository.handle(event: .providerCredentialStatus(
            operationId: listOperationID,
            configuredProviderIds: [],
            unavailableProviderIds: [openAI],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: "provider credential storage is unavailable"
        ))

        let openAIState = try XCTUnwrap(repository.state(for: openAI))
        let kimiState = try XCTUnwrap(repository.state(for: kimi))
        XCTAssertEqual(openAIState.credentialState, .configured)
        XCTAssertEqual(kimiState.credentialState, .missing)
    }

    func testCredentialListTimeoutClearsOnlyRecordedProfilesAndIgnoresLateEvent() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(
            persistenceURL: persistenceURL,
            credentialOperationTimeout: .milliseconds(20)
        )
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(operationID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected list provider credentials")
        }
        XCTAssertTrue(try XCTUnwrap(repository.state(for: openAI)).operationInFlight)

        try await Task.sleep(for: .milliseconds(60))

        let timedOutState = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertFalse(timedOutState.operationInFlight)
        XCTAssertEqual(repository.lastRepositoryError, "安全存储操作超时")
        let credentialBeforeLateEvent = timedOutState.credentialState

        repository.handle(event: .providerCredentialStatus(
            operationId: operationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))

        XCTAssertEqual(repository.state(for: openAI)?.credentialState, credentialBeforeLateEvent,
                       "a timed-out list response must be ignored")
    }

    func testCredentialListRefreshReplacesPreviousOperationAndIgnoresItsLateResponse() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(oldOperationID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected first list provider credentials")
        }
        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(currentOperationID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected replacement list provider credentials")
        }
        XCTAssertNotEqual(oldOperationID, currentOperationID)

        repository.handle(event: .providerCredentialStatus(
            operationId: oldOperationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        var state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .unknown)
        XCTAssertTrue(state.operationInFlight,
                      "the replacement operation still owns the in-flight state")

        repository.handle(event: .providerCredentialStatus(
            operationId: currentOperationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertFalse(state.operationInFlight)
    }

    func testSetDeleteCorrelationAndErrorHandling() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        repository.stageSecret("sk-openai", for: openAI)
        let failedApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)

        guard case let .setProviderCredential(operationId: setFailureID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setFailureID,
            configuredProviderIds: [],
            unavailableProviderIds: [openAI],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: "failed to store provider credential: locked"
        ))
        await failedApply.value

        var state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.detailMessage, "failed to store provider credential: locked")
        XCTAssertEqual(state.pendingSecret, "sk-openai")

        let successfulApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(2, recorder: recorder)
        guard case let .setProviderCredential(operationId: setSuccessID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected second set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setSuccessID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await successfulApply.value

        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertEqual(state.pendingSecret, "")

        repository.clearCredentialRequest(for: openAI)
        let deleteApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(3, recorder: recorder)
        guard case let .deleteProviderCredential(operationId: deleteID, providerId: deleteProviderID) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected delete provider credential")
        }
        XCTAssertEqual(deleteProviderID, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: deleteID,
            configuredProviderIds: [],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await deleteApply.value

        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .missing)
        XCTAssertFalse(state.clearCredentialOnApply)
    }

    func testCredentialMutationRequiresAuthoritativeConfirmation() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        repository.stageSecret("sk-openai", for: openAI)
        let setTask = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)
        guard case let .setProviderCredential(operationId: setID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setID,
            configuredProviderIds: [],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await setTask.value

        var state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .unknown)
        XCTAssertEqual(state.pendingSecret, "sk-openai")
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.detailMessage, "安全存储未确认密钥已保存。")

        let removeTask = Task { await repository.removeProfile(openAI) }
        await waitForCommandCount(2, recorder: recorder)
        guard case let .deleteProviderCredential(operationId: deleteID, providerId: providerID) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected delete provider credential")
        }
        XCTAssertEqual(providerID, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: deleteID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        let removed = await removeTask.value

        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertFalse(removed)
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.detailMessage, "安全存储仍报告该密钥存在。")
        XCTAssertNotNil(repository.state(for: openAI), "a profile must survive a failed secure deletion")
    }

    func testClearingDefaultCredentialKeepsAsyncResultOnOriginalProfileAfterReorder() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.configure(
            submitCommand: { command in
                try await recorder.submit(command: command)
            },
            applyReconnect: { _ in }
        )

        repository.stageSecret("sk-openai", for: openAI)
        let initialApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)
        guard case let .setProviderCredential(operationId: setID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await initialApply.value

        repository.clearCredentialRequest(for: openAI)
        let clearApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(2, recorder: recorder)
        guard case let .deleteProviderCredential(operationId: deleteID, providerId: providerID) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected delete provider credential")
        }
        XCTAssertEqual(providerID, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: deleteID,
            configuredProviderIds: [],
            unavailableProviderIds: [],
            storageEncrypted: true,
            credentialPreviews: [:],
            error: nil
        ))
        await clearApply.value

        let openAIState = try XCTUnwrap(repository.state(for: openAI))
        let kimiState = try XCTUnwrap(repository.state(for: kimi))
        XCTAssertFalse(openAIState.profile.enabled)
        XCTAssertFalse(openAIState.profile.isDefault)
        XCTAssertEqual(openAIState.detailMessage, "已应用配置并请求重连。")
        XCTAssertEqual(openAIState.connectionState, .idle)
        XCTAssertTrue(kimiState.profile.isDefault)
        XCTAssertNil(kimiState.detailMessage, "async completion must not leak onto the newly sorted default profile")
    }

    func testConnectionResultUsesProfileIDAfterProfilesReorder() async throws {
        let gate = ProviderConnectionTestGate()
        let started = expectation(description: "connection test started")
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.stageSecret("sk-openai", for: openAI)
        repository.configure(
            submitCommand: nil,
            testConnection: { _, _ in
                started.fulfill()
                return await gate.waitForResult()
            }
        )

        let testTask = Task { await repository.testConnection(openAI) }
        await fulfillment(of: [started])

        repository.setDefaultProfile(kimi)
        repository.updateProfile(kimi) { $0.name = "Kimi Default" }
        XCTAssertEqual(repository.profiles.first?.id, kimi, "test precondition: profile order must change while the request is suspended")

        await gate.succeed()
        await testTask.value

        let openAIState = try XCTUnwrap(repository.state(for: openAI))
        let kimiState = try XCTUnwrap(repository.state(for: kimi))
        XCTAssertEqual(openAIState.connectionState, .connected)
        XCTAssertEqual(openAIState.detailMessage, "连接测试成功。")
        XCTAssertEqual(kimiState.connectionState, .idle)
        XCTAssertNil(kimiState.detailMessage)
    }

    func testConnectionWithDraftCredentialWarnsThatKeyIsNotSaved() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        repository.stageSecret("sk-deepseek-draft", for: deepSeek)
        repository.configure(
            submitCommand: nil,
            testConnection: { _, credentialOverride in
                XCTAssertEqual(credentialOverride, "sk-deepseek-draft")
                return .success(
                    message: "连接测试成功。 · 12ms",
                    usedStoredCredential: false
                )
            }
        )

        await repository.testConnection(deepSeek)

        let state = try XCTUnwrap(repository.state(for: deepSeek))
        XCTAssertEqual(state.connectionState, .connected)
        XCTAssertEqual(
            state.detailMessage,
            "连接测试成功。 · 12ms" + String(localized: "settings_provider_key_unsaved_suffix")
        )
        XCTAssertEqual(state.pendingSecret, "sk-deepseek-draft")
        XCTAssertNotEqual(state.credentialState, .configured)
    }

    // MARK: legacy Anthropic Keychain migration

    /// Regression: `Keychain.model` holds the engine's ACTIVE model — written on
    /// every `ModelList`/`ModelChanged`, for every provider — so it is not
    /// evidence that an Anthropic provider was ever configured. Treating it as
    /// evidence fabricated an enabled, default Anthropic profile for a user who
    /// had configured nothing, and the composer chip then showed that profile's
    /// model as the default.
    func testStoredActiveModelAloneDoesNotFabricateAnAnthropicProfile() {
        XCTAssertNil(ProviderRepository.legacyAnthropicProfile(
            legacyKey: nil,
            legacyBase: nil,
            legacyModel: "deepseek/deepseek-flash"
        ))
        XCTAssertNil(ProviderRepository.legacyAnthropicProfile(
            legacyKey: nil,
            legacyBase: nil,
            legacyModel: "anthropic/claude-sonnet-5"
        ))
        XCTAssertNil(ProviderRepository.legacyAnthropicProfile(
            legacyKey: nil,
            legacyBase: nil,
            legacyModel: nil
        ))
    }

    /// A real legacy configuration (api key and/or base URL override) still
    /// migrates, and a bare stored model is still adopted.
    func testLegacyCredentialMigratesWithItsBareModelID() throws {
        let state = try XCTUnwrap(ProviderRepository.legacyAnthropicProfile(
            legacyKey: "sk-legacy",
            legacyBase: "https://proxy.example.com",
            legacyModel: "claude-sonnet-5"
        ))

        XCTAssertEqual(state.profile.id, "anthropic")
        XCTAssertEqual(state.profile.baseURL, "https://proxy.example.com")
        XCTAssertEqual(state.profile.modelID, "claude-sonnet-5")
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertTrue(state.hasLegacyAnthropicCredential)
    }

    /// An `anthropic/`-qualified stored model is un-qualified before it is
    /// stored, because `qualifiedModelID` re-adds the prefix on the way out.
    func testLegacyMigrationStripsItsOwnProviderQualifier() throws {
        let state = try XCTUnwrap(ProviderRepository.legacyAnthropicProfile(
            legacyKey: "sk-legacy",
            legacyBase: nil,
            legacyModel: "anthropic/claude-opus-4-8"
        ))

        // `qualifiedModelID` is `id + "/" + modelID`, so a bare modelID here is
        // exactly what makes the launch snapshot's ref singly-qualified.
        XCTAssertEqual(state.profile.modelID, "claude-opus-4-8")
    }

    /// Regression (the iOS "DeepSeek V4 Flash under ANTHROPIC" chip): a stored
    /// model belonging to ANOTHER provider must not be smuggled into the
    /// Anthropic profile — `qualifiedModelID` would double-qualify it into
    /// `anthropic/deepseek/deepseek-flash`, which the engine registered under
    /// the Anthropic profile and the picker rendered in Anthropic's section.
    func testLegacyMigrationRejectsAForeignProviderModel() throws {
        for foreign in [
            "deepseek/deepseek-flash",
            "openrouter/openrouter/auto",
            "anthropic/deepseek/deepseek-flash",
            // BARE foreign ids too: `ClientEvent::ModelList.current` is emitted
            // UNQUALIFIED whenever the session carries no `model_profile`, so
            // this is the shape `Keychain.model` actually ends up holding.
            // `anthropic/` + `deepseek-flash` clears the engine's "the
            // remainder must be a bare id" guard, so nothing downstream catches
            // it — the rejection has to happen here. Coverage is bounded by what
            // `Presets.llm` knows: a bare id no preset lists is indistinguishable
            // from a custom Anthropic-compatible proxy model and is adopted.
            "deepseek-flash",
            "gpt-5.6-sol",
            "gemini-3.7-flash",
            "kimi-k3",
        ] {
            let state = try XCTUnwrap(ProviderRepository.legacyAnthropicProfile(
                legacyKey: "sk-legacy",
                legacyBase: nil,
                legacyModel: foreign
            ))
            let preset = try XCTUnwrap(Presets.llm.first(where: { $0.id == "anthropic" }))

            XCTAssertEqual(state.profile.modelID, preset.models.first, "leaked \(foreign)")
            // `qualifiedModelID` prefixes `id + "/"`, so any slash left in
            // `modelID` becomes a double-qualified launch reference.
            XCTAssertFalse(
                state.profile.modelID.contains("/"),
                "double-qualified from \(foreign)")
        }
    }

    /// A bare id that no OTHER preset claims is still adopted — that is how a
    /// custom Anthropic-compatible proxy model survives the migration, and it is
    /// the behaviour the foreign-id rejection above must not overreach into.
    func testLegacyMigrationKeepsAnUnclaimedBareModelID() throws {
        let state = try XCTUnwrap(ProviderRepository.legacyAnthropicProfile(
            legacyKey: "sk-legacy",
            legacyBase: "https://proxy.example.com",
            legacyModel: "my-in-house-claude-proxy"
        ))

        XCTAssertEqual(state.profile.modelID, "my-in-house-claude-proxy")
    }

    /// The migration's fallback becomes the launch `defaultModelID`, so it has to
    /// be a model the engine still curates. A stale id (`claude-sonnet-4-5`) is
    /// registered only because it is the configured default and renders as a
    /// stray row above the real Anthropic shortlist — the same wart
    /// `MobileEngineConfig::default()` was moved off `claude-sonnet-4-20250514`
    /// to avoid. Mirrors `traits::is_curated_model`'s "anthropic" arm.
    func testTheAnthropicPresetOnlyOffersCuratedModels() throws {
        let preset = try XCTUnwrap(Presets.llm.first(where: { $0.id == "anthropic" }))
        let curated: Set<String> = [
            "claude-opus-5",
            "claude-fable-5-1",
            "claude-sonnet-5",
            "claude-haiku-4-5",
        ]

        XCTAssertEqual(preset.models.first, "claude-opus-5",
                       "must match traits::provider_default_model(\"anthropic\")")
        for model in preset.models {
            XCTAssertTrue(curated.contains(model), "\(model) is not curated by the engine")
        }
    }

    // MARK: - Reentrancy: an index captured before an `await` is stale after it

    /// Anthropic OAuth sign-in is withheld from the UI: Anthropic's
    /// "Authentication and credential use" policy reserves claude.ai OAuth for
    /// Claude Code and claude.ai themselves. This is a UI gate only — the
    /// engine coordinator, PKCE, token exchange and refresh driver are all
    /// still there, and removing "anthropic" from
    /// `ProviderRepository.hiddenOAuthLoginProviders` restores the button.
    ///
    /// The gate must be provider-scoped: ChatGPT has no API-key path
    /// (`isOAuthOnly`), so withholding its OAuth would strand the provider.
    func testAnthropicOAuthLoginIsWithheldWhileChatGPTStillSignsIn() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")
        let chatgpt = repository.addProfile(presetID: "openai-chatgpt")

        var loginsAttempted: [String] = []
        repository.configure(
            submitCommand: nil,
            oauthLogin: { provider in
                loginsAttempted.append(provider)
                return ProviderOAuthState(
                    provider: provider,
                    signedIn: true,
                    accountLabel: nil,
                    accountID: nil,
                    organizationID: nil,
                    fedramp: false
                )
            }
        )

        XCTAssertFalse(repository.oauthLoginAvailable(for: "anthropic"))
        XCTAssertTrue(repository.oauthLoginAvailable(for: "openai-chatgpt"))

        await repository.loginOAuth(for: anthropic)
        XCTAssertEqual(
            loginsAttempted,
            [],
            "no authorize request may be started for Anthropic"
        )
        XCTAssertNotEqual(
            repository.state(for: anthropic)?.oauthState?.signedIn,
            true,
            "the withheld provider must not end up signed in"
        )

        await repository.loginOAuth(for: chatgpt)
        XCTAssertEqual(
            loginsAttempted,
            ["openai-chatgpt"],
            "the gate is Anthropic-only; ChatGPT still signs in"
        )
    }

    /// The gate withholds LOGIN, not cleanup. An account that signed in before
    /// the gate keeps a working Logout, so its keychain credential never
    /// becomes an orphan the UI cannot reach.
    func testAWithheldProviderCanStillSignOut() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")

        var logoutsRequested: [String] = []
        repository.configure(
            submitCommand: nil,
            oauthLogout: { provider in logoutsRequested.append(provider) }
        )

        XCTAssertEqual(
            repository.oauthProvider(for: "anthropic"),
            "anthropic",
            "the provider mapping stays intact — only the login button is gated"
        )

        await repository.logoutOAuth(for: anthropic)
        XCTAssertEqual(logoutsRequested, ["anthropic"])
    }

    /// `loginOAuth` resolves the row index, then awaits the reconnect handler.
    /// This class is `@MainActor`, which serializes but does NOT freeze state
    /// across a suspension: a `removeProfile` landing inside that window shifts
    /// every later row down one, so a write through the pre-await index tags the
    /// WRONG profile (or traps when the array shrank past it). Every post-await
    /// write must re-resolve through `indexOfProfile(id:)`.
    func testLoginOAuthReResolvesTheRowAfterTheReconnectAwait() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let removed = repository.addProfile(presetID: "openai")
        let target = repository.addProfile(presetID: "openai-chatgpt")
        let bystander = repository.addProfile(presetID: "deepseek")

        // The stale index must point at a row that still EXISTS after the
        // removal, so the defect shows up as a wrong-row write rather than a
        // trap — a mis-tagged profile is the quieter, likelier production
        // symptom.
        let staleIndex = try XCTUnwrap(repository.profiles.firstIndex { $0.id == target })
        XCTAssertLessThan(staleIndex, repository.profiles.count - 1, "need a row after the target")
        let staleVictim = repository.profiles[staleIndex + 1].id
        XCTAssertEqual(staleVictim, bystander)

        repository.configure(
            submitCommand: nil,
            applyReconnect: { [weak repository] _ in
                // Runs while `loginOAuth` is suspended on this very await.
                await repository?.removeProfile(removed)
            },
            oauthLogin: { provider in
                ProviderOAuthState(
                    provider: provider,
                    signedIn: true,
                    accountLabel: nil,
                    accountID: nil,
                    organizationID: nil,
                    fedramp: false
                )
            }
        )

        await repository.loginOAuth(for: target)

        XCTAssertNil(repository.state(for: removed), "the concurrent removal must have landed")
        XCTAssertEqual(
            repository.state(for: target)?.detailMessage,
            String(localized: "settings_provider_oauth_login_success"),
            "the success message belongs to the profile that signed in"
        )
        XCTAssertNil(
            repository.state(for: bystander)?.detailMessage,
            "a stale pre-await index would have tagged \(bystander) instead"
        )
    }

    private func waitForCommandCount(_ count: Int, recorder: CommandRecorder) async {
        for _ in 0..<100 where recorder.commands.count < count {
            await Task.yield()
        }
        XCTAssertGreaterThanOrEqual(recorder.commands.count, count)
    }

    private func jsonObject(from string: String) throws -> [String: Any]? {
        let data = try XCTUnwrap(string.data(using: .utf8))
        return try JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}

@MainActor
private final class CommandRecorder {
    private(set) var commands: [ClientCommand] = []

    func submit(command: ClientCommand) async throws {
        commands.append(command)
    }
}

private actor ProviderConnectionTestGate {
    private var continuation: CheckedContinuation<ProviderConnectionTestResult, Never>?
    private var pendingResult: ProviderConnectionTestResult?

    func waitForResult() async -> ProviderConnectionTestResult {
        if let pendingResult {
            self.pendingResult = nil
            return pendingResult
        }
        return await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func succeed() {
        if let continuation {
            self.continuation = nil
            continuation.resume(returning: .success())
        } else {
            pendingResult = .success()
        }
    }
}
