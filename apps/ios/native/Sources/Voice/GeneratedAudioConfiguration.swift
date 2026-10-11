// Generated from resources/voice/audio-config-schema.json and audio-config-fixtures.json.
// Do not edit by hand; run node resources/voice/scripts/generate-audio-config.mjs.
import Foundation

public let audioConfigurationSchemaVersion = 4
public let audioLanguageAuto = "auto"
public let audioMinimumRate = 0.5
public let audioMaximumRate = 2.0
public let audioDefaultRate = 1.0

public struct AudioSource: RawRepresentable, Codable, Equatable, Hashable, Sendable {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public static let automatic = AudioSource(rawValue: "automatic")
    public static let system = AudioSource(rawValue: "system")
    public static let provider = AudioSource(rawValue: "provider")
    public static let offline = AudioSource(rawValue: "offline")

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        rawValue = try container.decode(String.self)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

public enum AudioProviderKind: String, Codable, Sendable { case recognition, speech }
public enum AudioReadiness: String, Codable, Sendable { case available, permissionRequired, denied, unavailable }
public enum AudioRouteStatus: String, Codable, Sendable { case ready, permissionRequired, unavailable, invalidRequest }
public enum AudioFallbackFailure: String, Codable, Sendable { case permission, unavailable, busy, cancelled, timeout, invalidRequest, noSpeech, nativeFailure }

public struct AudioVoiceSelection: Codable, Equatable, Sendable {
    public let source: AudioSource
    public let id: String
    public let modelId: String?
    public let profileId: String?

    public init(source: AudioSource, id: String, modelId: String? = nil, profileId: String? = nil) {
        self.source = source
        self.id = id
        self.modelId = modelId
        self.profileId = profileId
    }
}

public struct AudioCloudBinding: Codable, Equatable, Sendable {
    public var binding: String
    public var profileId: String?
    public var modelId: String?
    public init(binding: String = "follow_session", profileId: String? = nil, modelId: String? = nil) {
        self.binding = binding; self.profileId = profileId; self.modelId = modelId
    }
}

public struct AudioConversationPreference: Codable, Equatable, Sendable {
    public var mode: String
    public var interaction: String
    public var cloud: AudioCloudBinding
    public var voice: AudioVoiceSelection?
    public init(mode: String = "agent", interaction: String = "turn_based", cloud: AudioCloudBinding = AudioCloudBinding(), voice: AudioVoiceSelection? = nil) {
        self.mode = mode; self.interaction = interaction; self.cloud = cloud; self.voice = voice
    }
}

public struct AudioProviderContext: Sendable {
    public let profileId: String
    public init(profileId: String) { self.profileId = profileId }
}
public struct AudioProviderCapability: Sendable {
    public let profileId: String
    public let providerId: String
    public let kind: AudioProviderKind
    public let supported: Bool
    public let readiness: String
    public let defaultModelId: String?
    public let modelIds: [String?]
    public init(profileId: String, providerId: String, kind: AudioProviderKind, supported: Bool, readiness: String, defaultModelId: String?, modelIds: [String?]) {
        self.profileId = profileId; self.providerId = providerId; self.kind = kind; self.supported = supported; self.readiness = readiness; self.defaultModelId = defaultModelId; self.modelIds = modelIds
    }
}

public struct AudioRecognitionPreference: Codable, Equatable, Sendable {
    public var source: AudioSource
    public var offlineModelId: String?
    public var cloud: AudioCloudBinding
    public init(source: AudioSource, offlineModelId: String? = nil, cloud: AudioCloudBinding = AudioCloudBinding()) {
        self.source = source
        self.offlineModelId = offlineModelId
        self.cloud = cloud
    }
}

public struct AudioSpeechPreference: Codable, Equatable, Sendable {
    public var source: AudioSource
    public var offlineModelId: String?
    public var voice: AudioVoiceSelection?
    public var cloud: AudioCloudBinding
    public init(source: AudioSource, offlineModelId: String? = nil, voice: AudioVoiceSelection? = nil, cloud: AudioCloudBinding = AudioCloudBinding()) {
        self.source = source
        self.offlineModelId = offlineModelId
        self.voice = voice
        self.cloud = cloud
    }
}

public struct AudioConfigurationV4: Codable, Equatable, Sendable {
    public var schemaVersion: Int
    public var recognition: AudioRecognitionPreference
    public var speech: AudioSpeechPreference
    public var language: String
    public var rate: Double
    public var autoPlayReplies: Bool
    public var conversation: AudioConversationPreference

    public init(
        schemaVersion: Int = audioConfigurationSchemaVersion,
        recognition: AudioRecognitionPreference,
        speech: AudioSpeechPreference,
        language: String = audioLanguageAuto,
        rate: Double = audioDefaultRate,
        autoPlayReplies: Bool = false,
        conversation: AudioConversationPreference = AudioConversationPreference()
    ) {
        self.schemaVersion = schemaVersion
        self.recognition = recognition
        self.speech = speech
        self.language = language
        self.rate = rate
        self.autoPlayReplies = autoPlayReplies
        self.conversation = conversation
    }
}

public struct AudioOfflineModelAvailability: Equatable, Sendable {
    public let id: String
    public let kind: AudioProviderKind
    public let languages: [String]
    public let installed: Bool
    public let voiceIds: [String]?
    public init(id: String, kind: AudioProviderKind, languages: [String], installed: Bool, voiceIds: [String]? = nil) {
        self.id = id
        self.kind = kind
        self.languages = languages
        self.installed = installed
        self.voiceIds = voiceIds
    }
}

public protocol AudioRoutePreferenceProviding {
    var source: AudioSource { get }
    var offlineModelId: String? { get }
    var cloud: AudioCloudBinding { get }
    var routeVoice: AudioVoiceSelection? { get }
}

extension AudioRecognitionPreference: AudioRoutePreferenceProviding {
    public var routeVoice: AudioVoiceSelection? { nil }
}

extension AudioSpeechPreference: AudioRoutePreferenceProviding {
    public var routeVoice: AudioVoiceSelection? { voice }
}

public struct AudioRouteRequest: Sendable {
    public let kind: AudioProviderKind
    public let source: AudioSource
    public let offlineModelId: String?
    public let voice: AudioVoiceSelection?
    public let language: String
    public let systemStatus: AudioReadiness
    public let offlineModels: [AudioOfflineModelAvailability]
    public let systemVoiceIds: [String]?
    public let cloud: AudioCloudBinding
    public let sessionContext: AudioProviderContext?
    public let providerCapabilities: [AudioProviderCapability]

    public init<P: AudioRoutePreferenceProviding>(
        kind: AudioProviderKind,
        preference: P,
        language: String,
        systemStatus: AudioReadiness,
        offlineModels: [AudioOfflineModelAvailability],
        systemVoiceIds: [String]? = nil,
        voiceOverride: AudioVoiceSelection? = nil,
        sessionContext: AudioProviderContext? = nil,
        providerCapabilities: [AudioProviderCapability] = []
    ) {
        self.kind = kind
        self.source = preference.source
        self.offlineModelId = preference.offlineModelId
        self.voice = voiceOverride ?? preference.routeVoice
        self.language = language
        self.systemStatus = systemStatus
        self.offlineModels = offlineModels
        self.systemVoiceIds = systemVoiceIds
        self.cloud = preference.cloud
        self.sessionContext = sessionContext
        self.providerCapabilities = providerCapabilities
    }
}

public struct AudioRouteResolution: Equatable, Sendable {
    public struct Requested: Equatable, Sendable {
        public let source: AudioSource
        public let offlineModelId: String?
        public let voice: AudioVoiceSelection?
    }
    public struct Effective: Equatable, Sendable {
        public let source: AudioSource
        public let modelId: String?
        public let voiceId: String?
        public var profileId: String? = nil
        public var providerId: String? = nil
    }
    public let requested: Requested
    public let effective: Effective?
    public let status: AudioRouteStatus
    public let reason: String
    public let fallbackReason: String?
}

public enum AudioConfigurationNormalizer {
    public static var defaults: AudioConfigurationV4 {
        AudioConfigurationV4(
            recognition: AudioRecognitionPreference(source: .automatic),
            speech: AudioSpeechPreference(source: .automatic),
            language: audioLanguageAuto,
            rate: audioDefaultRate,
            autoPlayReplies: false
        )
    }

    public static func normalize(_ value: Any?) -> AudioConfigurationV4 {
        let raw = value as? [String: Any] ?? [:]
        guard raw["schemaVersion"] as? Int == audioConfigurationSchemaVersion else { return defaults }
        let recognitionRaw = raw["recognition"] as? [String: Any] ?? [:]
        let speechRaw = raw["speech"] as? [String: Any] ?? [:]
        let recognition = AudioRecognitionPreference(
            source: source(recognitionRaw["source"]),
            offlineModelId: modelID(recognitionRaw["offlineModelId"]),
            cloud: cloud(recognitionRaw["cloud"])
        )
        var speech = AudioSpeechPreference(
            source: source(speechRaw["source"]),
            offlineModelId: modelID(speechRaw["offlineModelId"]),
            voice: voice(speechRaw["voice"]),
            cloud: cloud(speechRaw["cloud"])
        )
        if speech.source == .automatic { speech.voice = nil }
        return AudioConfigurationV4(
            recognition: recognition,
            speech: speech,
            language: language(raw["language"]),
            rate: rate(raw["rate"]),
            autoPlayReplies: (raw["autoPlayReplies"] as? Bool) == true,
            conversation: conversation(raw["conversation"])
        )
    }

    private static func cloud(_ value: Any?) -> AudioCloudBinding {
        let raw = value as? [String: Any] ?? [:]
        return AudioCloudBinding(binding: raw["binding"] as? String ?? "follow_session", profileId: modelID(raw["profileId"]), modelId: modelID(raw["modelId"]))
    }
    private static func conversation(_ value: Any?) -> AudioConversationPreference {
        let raw = value as? [String: Any] ?? [:]
        return AudioConversationPreference(mode: raw["mode"] as? String ?? "agent", interaction: raw["interaction"] as? String ?? "turn_based", cloud: cloud(raw["cloud"]), voice: voice(raw["voice"]))
    }
    private static func source(_ value: Any?) -> AudioSource {
        let text = (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return AudioSource(rawValue: text.isEmpty ? "automatic" : text)
    }
    private static func modelID(_ value: Any?) -> String? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        return text
    }
    private static func language(_ value: Any?) -> String {
        let text = (value as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return text.isEmpty || text.lowercased() == audioLanguageAuto ? audioLanguageAuto : text
    }
    private static func rate(_ value: Any?) -> Double {
        let number = (value as? NSNumber)?.doubleValue ?? audioDefaultRate
        return min(audioMaximumRate, max(audioMinimumRate, number.isFinite ? number : audioDefaultRate))
    }
    private static func voice(_ value: Any?) -> AudioVoiceSelection? {
        guard let raw = value as? [String: Any], let id = raw["id"] as? String, !id.isEmpty else { return nil }
        let selectedSource = source(raw["source"])
        if selectedSource == .provider { return AudioVoiceSelection(source: selectedSource, id: id, modelId: modelID(raw["modelId"]), profileId: modelID(raw["profileId"])) }
        if selectedSource == .offline {
            return AudioVoiceSelection(source: selectedSource, id: id, modelId: modelID(raw["modelId"]))
        }
        return AudioVoiceSelection(source: selectedSource, id: id)
    }

}

public func resolveAudioLanguage(configured: String, deviceLocale: String?) -> String {
    let selected = configured.trimmingCharacters(in: .whitespacesAndNewlines)
    guard selected.isEmpty || selected.lowercased() == audioLanguageAuto else { return selected }
    let locale = deviceLocale?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return locale.isEmpty ? "en-US" : locale
}

private func audioLanguageMatches(_ supported: [String], _ requested: String) -> Bool {
    let language = requested.lowercased()
    return supported.contains { value in
        let candidate = value.lowercased()
        return language == candidate || language.hasPrefix(candidate + "-")
    }
}

private func audioRouteResult(
    _ request: AudioRouteRequest,
    _ effective: AudioRouteResolution.Effective?,
    _ status: AudioRouteStatus,
    _ reason: String,
    _ fallbackReason: String? = nil
) -> AudioRouteResolution {
    AudioRouteResolution(
        requested: .init(source: request.source, offlineModelId: request.offlineModelId, voice: request.voice),
        effective: effective,
        status: status,
        reason: reason,
        fallbackReason: fallbackReason
    )
}

private func audioUnavailable(_ request: AudioRouteRequest, _ reason: String, _ status: AudioRouteStatus = .unavailable, _ fallbackReason: String? = nil) -> AudioRouteResolution {
    audioRouteResult(request, nil, status, reason, fallbackReason)
}

private func audioSystemRoute(_ request: AudioRouteRequest, voice: AudioVoiceSelection?) -> AudioRouteResolution {
    if request.systemStatus == .available || request.systemStatus == .permissionRequired {
        let voiceID = voice?.id == "default" ? nil : voice?.id
        if let voiceID, let voices = request.systemVoiceIds, !voices.contains(voiceID) {
            return audioUnavailable(request, "systemVoiceUnknown")
        }
        let status: AudioRouteStatus = request.systemStatus == .available ? .ready : .permissionRequired
        return audioRouteResult(request, .init(source: .system, modelId: nil, voiceId: voiceID), status, status == .ready ? "ready" : "systemPermissionRequired")
    }
    return audioUnavailable(request, request.systemStatus == .denied ? "systemDenied" : "systemUnavailable")
}

private func audioOfflineRoute(_ request: AudioRouteRequest, modelId: String?, voice: AudioVoiceSelection?) -> AudioRouteResolution {
    let selected: AudioOfflineModelAvailability?
    if let modelId {
        guard let model = request.offlineModels.first(where: { $0.id == modelId }) else { return audioUnavailable(request, "offlineModelUnknown") }
        guard model.kind == request.kind else { return audioUnavailable(request, "offlineModelKindMismatch") }
        guard audioLanguageMatches(model.languages, request.language) else { return audioUnavailable(request, "offlineModelUnsupportedLanguage") }
        guard model.installed else { return audioUnavailable(request, "offlineModelNotInstalled") }
        selected = model
    } else {
        let compatible = request.offlineModels.filter { $0.kind == request.kind && audioLanguageMatches($0.languages, request.language) }
        guard let model = compatible.first(where: \.installed) else {
            return audioUnavailable(request, compatible.isEmpty ? "noCompatibleOfflineModel" : "offlineModelNotInstalled")
        }
        selected = model
    }
    guard let selected else { return audioUnavailable(request, "noCompatibleOfflineModel") }
    if let voice, voice.source == .offline {
        guard voice.modelId == selected.id else { return audioUnavailable(request, "offlineModelConflict", .invalidRequest) }
        if let voices = selected.voiceIds, !voices.contains(voice.id) { return audioUnavailable(request, "offlineVoiceUnknown") }
    }
    return audioRouteResult(request, .init(source: .offline, modelId: selected.id, voiceId: voice?.source == .offline ? voice?.id : nil), .ready, "ready")
}

public func resolveAudioRoute(_ request: AudioRouteRequest) -> AudioRouteResolution {
    if request.language.isEmpty || request.language.lowercased() == audioLanguageAuto { return audioUnavailable(request, "languageUnresolved", .invalidRequest) }
    let source = request.source
    let voice = request.kind == .speech ? request.voice : nil
    if source == .provider {
        let cloud = request.cloud
        guard cloud.binding == "follow_session" || cloud.binding == "explicit_profile" else { return audioUnavailable(request, "providerBindingInvalid", .invalidRequest) }
        guard let profileId = cloud.binding == "follow_session" ? request.sessionContext?.profileId : cloud.profileId else { return audioUnavailable(request, cloud.binding == "follow_session" ? "sessionProfileRequired" : "providerProfileRequired") }
        guard let capability = request.providerCapabilities.first(where: { $0.profileId == profileId && $0.kind == request.kind }), capability.supported else { return audioUnavailable(request, "providerOperationUnsupported") }
        guard capability.readiness == "ready" else { return audioUnavailable(request, capability.readiness == "unreachable" ? "providerUnreachable" : "providerConfigurationRequired") }
        let modelId = cloud.modelId ?? capability.defaultModelId
        guard capability.modelIds.contains(modelId) else { return audioUnavailable(request, "providerModelUnsupported") }
        if let voice, voice.source != .provider || voice.profileId != profileId || voice.modelId != modelId { return audioUnavailable(request, "providerVoiceScopeMismatch", .invalidRequest) }
        return AudioRouteResolution(requested: .init(source: request.source, offlineModelId: request.offlineModelId, voice: request.voice), effective: .init(source: .provider, modelId: modelId, voiceId: voice?.id, profileId: profileId, providerId: capability.providerId), status: .ready, reason: "ready", fallbackReason: nil)
    }
    guard source == .automatic || source == .system || source == .offline else { return audioUnavailable(request, "unsupportedSource") }
    if let voice, voice.source != .system && voice.source != .offline { return audioUnavailable(request, "unsupportedVoiceSource", .invalidRequest) }
    if let voice, source != .automatic && voice.source != source { return audioUnavailable(request, "voiceSourceMismatch", .invalidRequest) }
    if let voice, voice.source == .offline, let requestedModel = request.offlineModelId, requestedModel != voice.modelId {
        return audioUnavailable(request, "offlineModelConflict", .invalidRequest)
    }
    if source == .system || (source == .automatic && voice?.source == .system) {
        return audioSystemRoute(request, voice: voice?.source == .system ? voice : nil)
    }
    if source == .offline || (source == .automatic && voice?.source == .offline) {
        return audioOfflineRoute(request, modelId: voice?.source == .offline ? voice?.modelId : request.offlineModelId, voice: voice)
    }
    let system = audioSystemRoute(request, voice: nil)
    if system.status == .ready || system.status == .permissionRequired { return system }
    let offline = audioOfflineRoute(request, modelId: request.offlineModelId, voice: nil)
    return AudioRouteResolution(
        requested: offline.requested,
        effective: offline.effective,
        status: offline.status,
        reason: offline.reason,
        fallbackReason: system.reason
    )
}

public func isAudioFallbackAllowed(_ failure: AudioFallbackFailure, operationStarted: Bool) -> Bool {
    !operationStarted && (failure == .permission || failure == .unavailable)
}
