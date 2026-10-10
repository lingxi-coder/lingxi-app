import Foundation
import Observation

/// A trusted, native session identity. Audio never infers a profile from a
/// chat-model suffix or substitutes the repository's default profile.
struct IOSAudioSessionContext: Codable, Equatable, Sendable {
    let sessionId: String
    let profileId: String
    let accountScope: String?
}

@MainActor
protocol IOSAudioProviderHostDriving: AnyObject {
    func capabilities(requestJson: String) async throws -> String
    func transcribe(requestJson: String, audio: Data, mimeType: String) async throws -> String
    func synthesize(requestJson: String, text: String) async throws -> String
    func cancel(operationId: String) async throws
}

struct IOSCloudTranscript {
    let text: String
    let language: String?
    let confidence: Float?
}

struct IOSAudioUsageRecord: Equatable {
    let operationID: String
    let profileID: String
    let accountScope: String
    let modelID: String?
    let usageJSON: String
    var turnID: String? = nil
}

struct IOSRealtimeCapability {
    let supportsTruncation: Bool
    let supported: Bool
    let readiness: String
    let reason: String?
    let modelID: String?
    let models: [String?]
    let voices: [String]
}

struct IOSAudioCloudCapability {
    let route: AudioProviderCapability
    let voices: [String]
    let reason: String?
}

@Observable
@MainActor
final class IOSAudioProviderService {
    static let shared = IOSAudioProviderService()
    typealias HostBuilder = (String, String) throws -> any IOSAudioProviderHostDriving

    struct PinnedOperation {
        let id: String
        let host: any IOSAudioProviderHostDriving
        let requestJSON: String
        let profileID: String
        let modelID: String?
    }

    private var hostBuilder: HostBuilder? = { try IOSAudioProviderHost(profilesJSON: $0, region: $1) }
    private var host: (any IOSAudioProviderHostDriving)?
    private var sessionEngine: MobileEngineHandle?
    private var sessionHost: (any IOSAudioProviderHostDriving)?
    private var hostRegion: String?
    private var profilesJSON: String?
    private var sessionContextOwnerID: String?
    private var contextRegion: String?
    private var refreshEpoch: UInt64 = 0
    private var latestConfigurationRevision: UInt64?
    private var generation: UInt64 = 0
    private(set) var usageRecords: [IOSAudioUsageRecord] = []
    private(set) var sessionContext: IOSAudioSessionContext?
    private(set) var capabilities: [AudioProviderKind: IOSAudioCloudCapability] = [:]
    private(set) var realtimeCapability: IOSRealtimeCapability?
    private(set) var lastError: String?

    func installHostBuilder(_ builder: @escaping HostBuilder) {
        hostBuilder = builder
        sessionHost = nil
        host = nil
        profilesJSON = nil
        invalidateCapabilities()
    }

    func claimSessionContext(ownerID: String) {
        guard sessionContextOwnerID != ownerID else { return }
        sessionContextOwnerID = ownerID
        sessionContext = nil
        sessionEngine = nil
        sessionHost = nil
        contextRegion = nil
        usageRecords.removeAll()
        invalidateCapabilities()
    }

    func clearSessionContext(ownerID: String) {
        guard sessionContextOwnerID == ownerID else { return }
        sessionContext = nil
        sessionEngine = nil
        sessionHost = nil
        contextRegion = nil
        usageRecords.removeAll()
        invalidateCapabilities()
    }

    func attach(engine: MobileEngineHandle, ownerID: String) {
        guard sessionContextOwnerID == ownerID, sessionEngine !== engine else { return }
        sessionEngine = engine
        sessionHost = nil
        invalidateCapabilities()
    }

    func setSessionContext(sessionID: String?, profileID: String?, accountScope: String? = nil, region: String? = nil, ownerID: String? = nil) {
        if let ownerID, sessionContextOwnerID != ownerID { return }
        if let region, contextRegion != region {
            contextRegion = region
            host = nil
            profilesJSON = nil
            sessionHost = nil
            invalidateCapabilities()
        }
        let next: IOSAudioSessionContext?
        if let sessionID, !sessionID.isEmpty, let profileID, !profileID.isEmpty {
            next = IOSAudioSessionContext(sessionId: sessionID, profileId: profileID, accountScope: accountScope)
        } else { next = nil }
        guard next != sessionContext else { return }
        sessionContext = next
        sessionHost = nil
        invalidateCapabilities()
    }

    var routeContext: AudioProviderContext? {
        sessionContext.map { AudioProviderContext(profileId: $0.profileId) }
    }

    var routeCapabilities: [AudioProviderCapability] { capabilities.values.map(\.route) }

    func profileID(for binding: AudioCloudBinding) -> String? {
        switch binding.binding {
        case "follow_session": sessionContext?.profileId
        case "explicit_profile": binding.profileId
        default: nil
        }
    }

    func refresh(snapshot: AudioConfigurationSnapshot) async {
        let bindings = [snapshot.configuration.recognition.cloud, snapshot.configuration.speech.cloud, snapshot.configuration.conversation.cloud]
        if bindings.contains(where: { $0.binding == "explicit_profile" }) {
            // Refresh repository identity before allocating this request epoch.
            // A broken explicit preview must not block the engine-bound route.
            _ = try? obtainHost()
        }
        if let latestConfigurationRevision, snapshot.revision < latestConfigurationRevision { return }
        if latestConfigurationRevision != snapshot.revision {
            capabilities.removeAll()
            realtimeCapability = nil
        }
        latestConfigurationRevision = snapshot.revision
        refreshEpoch &+= 1
        let epoch = refreshEpoch
        let version = generation
        for kind in [AudioProviderKind.recognition, .speech] {
            let binding = kind == .recognition ? snapshot.configuration.recognition.cloud : snapshot.configuration.speech.cloud
            let profileID = profileID(for: binding)
            do {
                let request = try requestJSON(binding: binding, kind: kind.rawValue, operationID: UUID().uuidString.lowercased(), snapshot: snapshot)
                let host = try obtainHost(binding: binding)
                let response = try await host.capabilities(requestJson: request)
                guard version == generation, epoch == refreshEpoch, latestConfigurationRevision == snapshot.revision else { return }
                capabilities[kind] = try Self.parseCapability(response, kind: kind, profileID: profileID)
                lastError = nil
            } catch {
                guard version == generation, epoch == refreshEpoch, latestConfigurationRevision == snapshot.revision else { return }
                capabilities.removeValue(forKey: kind)
                lastError = error.localizedDescription
            }
        }
        do {
            let request = try requestJSON(binding: snapshot.configuration.conversation.cloud, kind: "realtime", operationID: UUID().uuidString.lowercased(), snapshot: snapshot, voice: snapshot.configuration.conversation.voice?.id)
            let host = try obtainHost(binding: snapshot.configuration.conversation.cloud)
            let response = try Self.object(try await host.capabilities(requestJson: request))
            guard version == generation, epoch == refreshEpoch, latestConfigurationRevision == snapshot.revision else { return }
            realtimeCapability = Self.parseRealtimeCapability(response)
        } catch {
            guard version == generation, epoch == refreshEpoch, latestConfigurationRevision == snapshot.revision else { return }
            realtimeCapability = IOSRealtimeCapability(supportsTruncation: false, supported: false, readiness: "unavailable", reason: error.localizedDescription, modelID: nil, models: [], voices: [])
        }
        IOSAudioService.shared.publishCapabilitySnapshot()
    }

    func pinRealtime(snapshot: AudioConfigurationSnapshot) async throws -> (host: IosAudioProviderHost, requestJSON: String) {
        guard snapshot.configuration.conversation.interaction == "turn_based" else {
            throw AudioServiceFailure.nativeFailure("Interruptible realtime capture is unavailable. Select turn-based conversation.")
        }
        let binding = snapshot.configuration.conversation.cloud
        guard let profileID = profileID(for: binding), !profileID.isEmpty else { throw AudioServiceFailure.unavailable }
        let transport = try obtainHost(binding: binding)
        guard let adapter = transport as? IOSAudioProviderHost else { throw AudioServiceFailure.unsupported }
        let request = try requestJSON(binding: binding, kind: "realtime", operationID: UUID().uuidString.lowercased(), snapshot: snapshot, voice: snapshot.configuration.conversation.voice?.id)
        var object = try Self.object(try await transport.capabilities(requestJson: request))
        let capability = Self.parseRealtimeCapability(object)
        let modelID = binding.modelId ?? capability.modelID
        guard capability.supported, capability.readiness == "ready", capability.models.contains(modelID) else {
            throw AudioServiceFailure.nativeFailure(capability.reason ?? "Native realtime audio is unavailable for this profile.")
        }
        if let voice = snapshot.configuration.conversation.voice {
            guard voice.source == .provider, voice.profileId == profileID, voice.modelId == modelID,
                  capability.voices.contains(voice.id) else { throw AudioServiceFailure.voiceMissing }
        }
        object = try Self.object(request)
        var cloud = object["cloud"] as? [String: Any] ?? [:]
        cloud["modelId"] = modelID as Any? ?? NSNull()
        object["cloud"] = cloud
        return (adapter.host, String(decoding: try JSONSerialization.data(withJSONObject: object, options: .sortedKeys), as: UTF8.self))
    }

    private static func parseRealtimeCapability(_ object: [String: Any]) -> IOSRealtimeCapability {
        let values = object["models"] as? [[String: Any]] ?? []
        let modelID = object["modelId"] as? String
        let selected = values.first { $0["id"] as? String == modelID }
        let voices = (selected?["voices"] as? [[String: Any]])?.compactMap { $0["id"] as? String } ?? []
        return IOSRealtimeCapability(supportsTruncation: (object["capabilities"] as? [String: Any])?["audioTruncation"] as? Bool ?? false, supported: object["supported"] as? Bool ?? false, readiness: object["readiness"] as? String ?? "unavailable", reason: object["reason"] as? String, modelID: modelID, models: values.map { $0["id"] as? String }, voices: voices)
    }

    func pin(kind: AudioProviderKind, snapshot: AudioConfigurationSnapshot, voice: String? = nil, owner: IOSAudioOwner? = nil, operationID: String? = nil, maximumBytes: UInt64? = nil, timeoutBudgetMs: UInt64? = nil) async throws -> PinnedOperation {
        let binding = kind == .recognition ? snapshot.configuration.recognition.cloud : snapshot.configuration.speech.cloud
        try validateOwner(owner, binding: binding)
        let host = try obtainHost(binding: binding)
        guard let profile = profileID(for: binding), !profile.isEmpty else {
            throw AudioServiceFailure.nativeFailure("Choose a provider profile or open a session before using cloud audio.")
        }
        let id = operationID ?? UUID().uuidString.lowercased()
        let request = try requestJSON(binding: binding, kind: kind.rawValue, operationID: id, snapshot: snapshot, voice: voice, maximumBytes: maximumBytes, timeoutBudgetMs: timeoutBudgetMs)
        let capability = try Self.parseCapability(try await host.capabilities(requestJson: request), kind: kind, profileID: profile)
        guard capability.route.supported else { throw AudioServiceFailure.unsupported }
        guard capability.route.readiness == "ready" else {
            throw AudioServiceFailure.nativeFailure(capability.reason ?? "The selected audio profile is unavailable. Check its credentials and connection.")
        }
        let model = binding.modelId ?? capability.route.defaultModelId
        guard capability.route.modelIds.contains(model) else { throw AudioServiceFailure.unsupported }
        if kind == .speech, let selected = snapshot.configuration.speech.voice {
            guard selected.source == .provider, selected.profileId == profile, selected.modelId == model,
                  capability.voices.contains(selected.id) else { throw AudioServiceFailure.voiceMissing }
        }
        var pinnedRequest = try Self.object(request)
        var cloud = pinnedRequest["cloud"] as? [String: Any] ?? [:]
        // Catalog defaults are materialized once, before capture or synthesis.
        cloud["modelId"] = model as Any? ?? NSNull()
        pinnedRequest["cloud"] = cloud
        let pinnedJSON = String(decoding: try JSONSerialization.data(withJSONObject: pinnedRequest, options: .sortedKeys), as: UTF8.self)
        return PinnedOperation(id: id, host: host, requestJSON: pinnedJSON, profileID: profile, modelID: model)
    }

    func transcribe(_ operation: PinnedOperation, recording: IOSAudioRecording) async throws -> IOSCloudTranscript {
        try await withTaskCancellationHandler {
            let response = try await operation.host.transcribe(requestJson: operation.requestJSON, audio: recording.audioBytes, mimeType: recording.mimeType)
            let object = try Self.object(response)
            recordUsage(object, operation: operation)
            try Task.checkCancellation()
            guard let text = object["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw AudioServiceFailure.noSpeech
            }
            let confidence = (object["confidence"] as? NSNumber)?.floatValue
            return IOSCloudTranscript(text: text, language: object["language"] as? String,
                confidence: confidence.flatMap { $0.isFinite && (0 ... 1).contains($0) ? $0 : nil })
        } onCancel: {
            Task { @MainActor in try? await operation.host.cancel(operationId: operation.id) }
        }
    }

    func synthesize(_ operation: PinnedOperation, text: String, maximumBytes: UInt64) async throws -> AudioPcmOutput {
        try await withTaskCancellationHandler {
            let response = try await operation.host.synthesize(requestJson: operation.requestJSON, text: text)
            let object = try Self.object(response)
            recordUsage(object, operation: operation)
            try Task.checkCancellation()
            guard let base64 = object["pcmBase64"] as? String,
                  UInt64(base64.utf8.count) <= (maximumBytes + 2) / 3 * 4,
                  let pcm = Data(base64Encoded: base64),
                  let rate = object["sampleRateHz"] as? NSNumber else {
                throw AudioServiceFailure.nativeFailure("The provider returned an invalid audio payload.")
            }
            _ = try IOSPcmPlayback.wave(pcm: pcm, sampleRateHz: rate.uint32Value, maximumBytes: maximumBytes)
            return AudioPcmOutput(pcm: pcm, sampleRateHz: rate.uint32Value)
        } onCancel: {
            Task { @MainActor in try? await operation.host.cancel(operationId: operation.id) }
        }
    }

    func validateOwner(_ owner: IOSAudioOwner?, binding: AudioCloudBinding) throws {
        guard binding.binding == "follow_session", let owner else { return }
        guard case let .session(sessionID) = owner, sessionContext?.sessionId == sessionID, sessionContext?.accountScope?.isEmpty == false else {
            throw AudioServiceFailure.nativeFailure("This audio owner has no matching trusted session route. Choose an explicit provider profile.")
        }
    }

    private func recordUsage(_ response: [String: Any], operation: PinnedOperation) {
        guard let raw = response["usage"] as? [String: Any] else { return }
        let context = response["usageContext"] as? [String: Any] ?? [:]
        let request = (try? Self.object(operation.requestJSON)) ?? [:]
        let binding = (request["cloud"] as? [String: Any])?["binding"] as? String
        let account = context["accountScope"] as? String ?? (binding == "follow_session" ? (request["session"] as? [String: Any])?["accountScope"] as? String : nil) ?? "profile:\(operation.profileID)"
        appendUsage(raw, operationID: operation.id, profileID: context["profileId"] as? String ?? operation.profileID,
            accountScope: account, modelID: (context["modelId"] as? String).flatMap { $0.isEmpty ? nil : $0 })
    }

    func recordRealtimeUsage(_ json: String) {
        guard let data = json.data(using: .utf8), data.count <= 1_500_000,
              let event = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any], event["type"] as? String == "usage",
              let context = event["usageContext"] as? [String: Any],
              let operationID = context["operationId"] as? String, !operationID.isEmpty,
              let profileID = context["profileId"] as? String, !profileID.isEmpty,
              let accountScope = context["accountScope"] as? String, !accountScope.isEmpty else { return }
        var usage: [String: Any] = [:]
        for key in ["inputTokens", "outputTokens", "totalTokens", "native"] { if let value = event[key] { usage[key] = value } }
        let turnID = event["turnId"].flatMap { value -> String? in
            if let text = value as? String { return text }
            return (value as? NSNumber)?.stringValue
        }
        appendUsage(usage, operationID: operationID, profileID: profileID, accountScope: accountScope, modelID: context["modelId"] as? String, turnID: turnID)
    }

    private func appendUsage(_ raw: [String: Any], operationID: String, profileID: String, accountScope: String, modelID: String?, turnID: String? = nil) {
        func numericOnly(_ value: Any, depth: Int = 0) -> Any? {
            guard depth <= 4 else { return nil }
            if let number = value as? NSNumber { return number.doubleValue.isFinite ? number : nil }
            guard let object = value as? [String: Any], object.count <= 64 else { return nil }
            let clean = object.reduce(into: [String: Any]()) { result, entry in
                guard entry.key.utf8.count <= 64, !entry.key.lowercased().contains("text") else { return }
                if let value = numericOnly(entry.value, depth: depth + 1) { result[entry.key] = value }
            }
            return clean.isEmpty ? nil : clean
        }
        guard let clean = numericOnly(raw), let data = try? JSONSerialization.data(withJSONObject: clean, options: .sortedKeys), data.count <= 16 * 1_024 else { return }
        var record = IOSAudioUsageRecord(operationID: operationID, profileID: profileID, accountScope: accountScope,
            modelID: modelID, usageJSON: String(decoding: data, as: UTF8.self))
        record.turnID = turnID
        usageRecords.removeAll { $0.operationID == operationID && $0.turnID == turnID }
        usageRecords.append(record)
        if usageRecords.count > 128 { usageRecords.removeFirst(usageRecords.count - 128) }
    }

    private func obtainHost(binding: AudioCloudBinding? = nil) throws -> any IOSAudioProviderHostDriving {
        if binding?.binding == "follow_session" {
            guard let sessionEngine, let context = sessionContext,
                  let accountScope = context.accountScope, !accountScope.isEmpty else {
                throw AudioServiceFailure.nativeFailure("Open a session before using its cloud audio profile.")
            }
            if let sessionHost { return sessionHost }
            let built = IOSAudioProviderHost(engine: sessionEngine)
            sessionHost = built
            return built
        }
        let snapshot = ProviderRepository.shared.makeLaunchSnapshot()
        let configuredRegion = contextRegion ?? (DesktopSettingsRepository.shared.effective["providerRegion"] as? String)
        let region = configuredRegion == "china_mainland" || configuredRegion == "china" ? "china" : "international"
        if profilesJSON != snapshot.providerProfilesJSON || hostRegion != region {
            hostRegion = region
            host = nil
            profilesJSON = snapshot.providerProfilesJSON
            invalidateCapabilities()
        }
        if let host { return host }
        guard let hostBuilder else { throw AudioServiceFailure.nativeFailure("The cloud audio host is unavailable in this build.") }
        let built = try hostBuilder(snapshot.providerProfilesJSON, region)
        host = built
        return built
    }

    private func invalidateCapabilities() {
        generation &+= 1
        refreshEpoch &+= 1
        latestConfigurationRevision = nil
        capabilities.removeAll()
        realtimeCapability = nil
    }

    private func requestJSON(binding: AudioCloudBinding, kind: String, operationID: String, snapshot: AudioConfigurationSnapshot, voice: String? = nil, maximumBytes: UInt64? = nil, timeoutBudgetMs: UInt64? = nil) throws -> String {
        var object: [String: Any] = [
            "operationId": operationID,
            "kind": kind,
            "cloud": ["binding": binding.binding, "profileId": binding.profileId as Any? ?? NSNull(), "modelId": binding.modelId as Any? ?? NSNull()],
            "language": resolveAudioLanguageForNativeDevice(configured: snapshot.configuration.language),
            "timeoutMs": min(timeoutBudgetMs ?? 120_000, 120_000),
            "maxPayloadBytes": maximumBytes ?? IOSAudioService.shared.maximumPayloadBytes ?? maxAudioPayloadBytes(),
        ]
        if let context = sessionContext, let accountScope = context.accountScope, !accountScope.isEmpty {
            object["session"] = ["sessionId": context.sessionId, "profileId": context.profileId, "accountScope": accountScope]
        }
        if kind == "speech" { object["rate"] = snapshot.configuration.rate }
        if kind == "realtime" { object["interaction"] = snapshot.configuration.conversation.interaction }
        if let voice { object["voice"] = voice }
        let data = try JSONSerialization.data(withJSONObject: object, options: .sortedKeys)
        return String(decoding: data, as: UTF8.self)
    }

    private static func object(_ json: String) throws -> [String: Any] {
        guard let data = json.data(using: .utf8), let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw AudioServiceFailure.nativeFailure("The audio host returned an invalid response.")
        }
        if let error = object["error"] as? String { throw AudioServiceFailure.nativeFailure(error) }
        if let error = object["error"] as? [String: Any] {
            if error["kind"] as? String == "unsupported" { throw AudioServiceFailure.unsupported }
            throw AudioServiceFailure.nativeFailure(error["message"] as? String ?? "The audio provider failed.")
        }
        return object
    }

    static func parseCapability(_ json: String, kind: AudioProviderKind, profileID: String?) throws -> IOSAudioCloudCapability {
        let object = try object(json)
        let modelValues = (object["models"] as? [[String: Any]]) ?? []
        let models = modelValues.map { $0["id"] as? String }
        let selectedModelID = object["modelId"] as? String
        let selectedModels = modelValues.filter { ($0["id"] as? String) == selectedModelID }
        let voices = selectedModels.flatMap { model -> [String] in
            return (model["voices"] as? [[String: Any]])?.compactMap { $0["id"] as? String } ?? []
        }
        return IOSAudioCloudCapability(
            route: AudioProviderCapability(
                profileId: object["profileId"] as? String ?? profileID ?? "",
                providerId: object["providerId"] as? String ?? "",
                kind: kind,
                supported: object["supported"] as? Bool ?? false,
                readiness: object["readiness"] as? String ?? "unavailable",
                defaultModelId: object["modelId"] as? String,
                modelIds: models
            ),
            voices: voices,
            reason: object["reason"] as? String
        )
    }
}
