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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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


    func testVisionDelegationDefaultsEnabledAndPersists() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)

        XCTAssertTrue(repository.makeLaunchSnapshot().visionDelegationEnabled)

        repository.setVisionDelegationEnabled(false)

        XCTAssertFalse(repository.makeLaunchSnapshot().visionDelegationEnabled)
        let reloaded = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        XCTAssertFalse(reloaded.makeLaunchSnapshot().visionDelegationEnabled)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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

    func testCustomAnthropicEndpointPreservesConfigurationAndWithholdsConsumerOAuth() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")
        repository.updateProfile(anthropic) { $0.baseURL = "https://proxy.example.test/anthropic" }
        let profile = try XCTUnwrap(repository.state(for: anthropic)?.profile)
        XCTAssertEqual(profile.baseURL, "https://proxy.example.test/anthropic")
        XCTAssertEqual(repository.engineProfileID(for: profile), "anthropic-user")
        XCTAssertFalse(repository.accountOAuthAvailable(for: profile))
        XCTAssertTrue(repository.requiresSavedAccountBinding(for: profile))
    }

    func testLaunchSnapshotCreatesIndependentProfileForCustomOfficialPresetEndpoint() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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

    func testEnabledProfileConfigurationSavesWithoutAnAccount() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")

        await repository.applyChanges(openAI)

        let state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.connectionState, .idle)
        XCTAssertNil(state.validationMessage)
        XCTAssertTrue(state.profile.enabled)
        XCTAssertNil(repository.lastRepositoryError)
    }

    func testDeepSeekPresetMatchesAndroidOfficialModels() throws {
        let preset = try XCTUnwrap(Presets.llm.first(where: { $0.id == "deepseek" }))

        XCTAssertEqual(preset.defaultUrl, "https://api.deepseek.com")
        XCTAssertEqual(preset.models, ["deepseek-flash", "deepseek-v4-pro"])

        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        let profile = try XCTUnwrap(repository.state(for: deepSeek)?.profile)
        XCTAssertEqual(profile.baseURL, "https://api.deepseek.com")
        XCTAssertEqual(profile.modelID, "deepseek-flash")
    }

    func testDeepSeekOfficialEndpointDoesNotDuplicateBuiltInEngineProfile() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        repository.setDefaultProfile(deepSeek)

        let snapshot = repository.makeLaunchSnapshot()

        XCTAssertEqual(snapshot.defaultModelID, "deepseek/deepseek-flash")
        XCTAssertEqual(snapshot.enabledProfileIDs, ["deepseek"])
        XCTAssertEqual(try jsonObject(from: snapshot.providerProfilesJSON)?.count, 0)
    }

    func testRoutingJsonIncludesRetryAndOrderedFallbackForDefaultModel() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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

    func testNewEnabledDraftPublishesOnApplyWithoutAnAccount() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        var draft = repository.makeNewDraft(presetID: "openai")
        draft.profile.name = "Unpublished OpenAI"

        XCTAssertNil(repository.state(for: draft.id))
        XCTAssertFalse(repository.makeLaunchSnapshot().profiles.contains { $0.settingsID == draft.id })

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertTrue(try XCTUnwrap(repository.state(for: draft.id)).profile.enabled)
        let reloaded = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        XCTAssertTrue(try XCTUnwrap(reloaded.state(for: draft.id)).profile.enabled)
    }

    func testOpenAIConfigurationSaveDoesNotRequireAnAPIAccountWhenOAuthUsesAnotherBinding() async throws {
        let accounts = ProviderAccountRepository()
        let oauth = ProviderAccount(accountId: "oauth-only", providerId: "openai", displayName: "Subscription",
            authKind: "oauth", executionBindingId: "openai-chatgpt", enabled: true, userPriority: 0,
            configRevision: 1, identityGeneration: 1)
        let snapshot = ProviderAccountSnapshot(storeScope: "fixture", configRevision: 1, accounts: [oauth], health: [])
        let json = String(decoding: try JSONEncoder().encode(snapshot), as: UTF8.self)
        var accountWrites = 0
        accounts.configure(load: { json }, command: { _, _ in accountWrites += 1; return json })
        await accounts.refresh()
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL, accounts: accounts)
        let draft = repository.makeNewDraft(presetID: "openai")
        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertTrue(try XCTUnwrap(repository.state(for: draft.id)).profile.enabled)
        XCTAssertEqual(accounts.snapshot.accounts.map(\.executionBindingId), ["openai-chatgpt"])
        XCTAssertEqual(accountWrites, 0, "Saving definitions never performs account lifecycle operations")
    }

    func testNewDraftUsesAConversationalDefaultModelWhenCatalogStartsWithNonChatModel() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)

        let draft = repository.makeNewDraft(presetID: "openai")

        XCTAssertEqual(draft.profile.modelID, "gpt-5.6-sol")
    }


    func testApplyDraftKeepsSavedProfileWhenReconnectFailsAfterPersistence() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        var draft = repository.makeNewDraft(presetID: "openai")
        draft.profile.name = "Saved before reconnect"
        draft.profile.enabled = false
        repository.configure(
            submitCommand: nil,
            applyReconnect: { _ in
                throw NSError(domain: "ProviderRepositoryTests", code: 1)
            },
            resetCatalog: false
        )

        let applied = await repository.applyDraft(draft)
        XCTAssertFalse(applied)
        XCTAssertEqual(repository.state(for: draft.id)?.profile.name, "Saved before reconnect")
        XCTAssertEqual(repository.state(for: draft.id)?.connectionState, .failed)

        let reloaded = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        XCTAssertEqual(reloaded.state(for: draft.id)?.profile.name, "Saved before reconnect")
    }

    func testApplyDraftVisibilityToggleOnlySkipsReconnectAndKeepsLaunchSnapshot() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
            },
            resetCatalog: false
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.profile.showInModelPicker = false

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertTrue(reconnectSnapshots.isEmpty)
        XCTAssertEqual(repository.makeLaunchSnapshot(), before)
        XCTAssertFalse(try XCTUnwrap(repository.state(for: id)).profile.showInModelPicker)

        let reloaded = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        XCTAssertFalse(try XCTUnwrap(reloaded.state(for: id)).profile.showInModelPicker)
    }

    func testApplyDraftVisibleModelAllowlistOnlySkipsReconnectAndKeepsLaunchSnapshot() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
            },
            resetCatalog: false
        )

        var draft = try XCTUnwrap(repository.makeDraft(for: id))
        draft.profile.visibleModelIDs = ["gpt-5.6-sol"]

        let applied = await repository.applyDraft(draft)
        XCTAssertTrue(applied)
        XCTAssertTrue(reconnectSnapshots.isEmpty)
        XCTAssertEqual(repository.makeLaunchSnapshot(), before)
        XCTAssertEqual(try XCTUnwrap(repository.state(for: id)).profile.visibleModelIDs, ["gpt-5.6-sol"])

        let reloaded = providerConfigurationTestRepository(persistenceURL: persistenceURL)
        XCTAssertEqual(try XCTUnwrap(reloaded.state(for: id)).profile.visibleModelIDs, ["gpt-5.6-sol"])
    }

    func testApplyDraftMasterTogglePreservesExplicitModelSelectionWhenTurnedBackOn() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
            },
            resetCatalog: false
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
            },
            resetCatalog: false
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

    #if canImport(harness_runtimeFFI)
    #endif

    func testEditingAndDiscardingDraftDoesNotChangeSavedSnapshot() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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

    func testPublishedCatalogReloadsHealthyAccountModelProjectionWithoutACommand() async throws {
        func accountSnapshot(models: [String]) throws -> String {
            let availability: [[String: Any]] = models.map {
                ["providerId": "openai", "executionBindingId": "openai", "modelId": $0,
                 "eligibleAccountIds": ["primary"], "confirmedAccountIds": [],
                 "unverifiedAccountIds": ["primary"], "forbiddenAccountIds": []]
            }
            return String(decoding: try JSONSerialization.data(withJSONObject: [
                "storeScope": "catalog-test", "configRevision": 1,
                "accounts": [["accountId": "primary", "providerId": "openai", "displayName": "Primary",
                    "authKind": "api_key", "executionBindingId": "openai", "enabled": true,
                    "userPriority": 0, "configRevision": 1, "identityGeneration": 1]],
                "health": [["accountId": "primary", "identityGeneration": 1, "status": "ready", "healthRevision": 1]],
                "modelEntitlements": [], "modelAvailability": availability, "scopedHealth": [],
            ]), as: UTF8.self)
        }
        let before = try accountSnapshot(models: ["old-model"])
        let after = try accountSnapshot(models: ["old-model", "new-model"])
        var reads = 0
        var commands = 0
        let accounts = ProviderAccountRepository()
        accounts.configure(load: { reads += 1; return reads == 1 ? before : after },
            command: { _, _ in commands += 1; return after })
        await accounts.refresh()
        let repository = ProviderRepository(persistenceURL: persistenceURL, accountRepository: accounts)
        XCTAssertFalse(accounts.modelIsAvailable(reference: "openai/new-model"))
        repository.acceptCatalog([ProviderCatalogEntry(id: "openai", displayName: "OpenAI",
            baseURL: "https://api.openai.com/v1", protocolName: "open_ai_responses", authName: "api_key",
            credentialEnv: "OPENAI_API_KEY", models: ["old-model", "new-model"], modelDetails: [:])])
        for _ in 0..<100 where reads < 2 { await Task.yield() }
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(reads, 2)
        XCTAssertEqual(commands, 0, "Catalog publication only refreshes Host metadata")
        XCTAssertTrue(accounts.modelIsAvailable(reference: "openai/new-model"))
        XCTAssertEqual(repository.visibleModelReferences(["openai/new-model"]), ["openai/new-model"])
        XCTAssertEqual(accounts.snapshot.accounts.map(\.accountId), ["primary"])
        XCTAssertEqual(accounts.snapshot.accounts.map(\.userPriority), [0])
    }

    func testCatalogRefreshKeepsOnlyKnownBuiltInPresetsInAddPicker() async throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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

        XCTAssertEqual(repository.catalogPresets.map(\.id), ["engine-only", "custom"])
        XCTAssertEqual(repository.preset(for: "engine-only").models, ["engine-model"])
        XCTAssertEqual(repository.preset(for: "engine-only").defaultUrl, "https://engine.example.test/v1")
    }

    func testEditorModelCatalogUsesLoadingStateUntilBuiltInCatalogArrives() throws {
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
        let repository = providerConfigurationTestRepository(persistenceURL: persistenceURL)
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
            },
            resetCatalog: false
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
        XCTAssertEqual(repository.routingMessage, String(localized: "settings_provider_routing_applied_reconnect"))
    }

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

    private func jsonObject(from string: String) throws -> [String: Any]? {
        let data = try XCTUnwrap(string.data(using: .utf8))
        return try JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}
