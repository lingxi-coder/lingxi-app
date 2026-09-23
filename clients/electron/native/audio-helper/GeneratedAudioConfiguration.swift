// Generated from clients/voice/audio-config-schema.json and audio-config-fixtures.json.
// Do not edit by hand; run node clients/voice/scripts/generate-audio-config.mjs.
import Foundation

public let audioConfigurationSchemaVersion = 3
public let audioLanguageAuto = "auto"
public let audioMinimumRate = 0.5
public let audioMaximumRate = 2.0
public let audioDefaultRate = 1.0

public struct AudioSource: RawRepresentable, Codable, Equatable, Hashable, Sendable {
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public static let automatic = AudioSource(rawValue: "automatic")
    public static let system = AudioSource(rawValue: "system")
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

    public init(source: AudioSource, id: String, modelId: String? = nil) {
        self.source = source
        self.id = id
        self.modelId = modelId
    }
}

public struct AudioRecognitionPreference: Codable, Equatable, Sendable {
    public var source: AudioSource
    public var offlineModelId: String?
    public init(source: AudioSource, offlineModelId: String? = nil) {
        self.source = source
        self.offlineModelId = offlineModelId
    }
}

public struct AudioSpeechPreference: Codable, Equatable, Sendable {
    public var source: AudioSource
    public var offlineModelId: String?
    public var voice: AudioVoiceSelection?
    public init(source: AudioSource, offlineModelId: String? = nil, voice: AudioVoiceSelection? = nil) {
        self.source = source
        self.offlineModelId = offlineModelId
        self.voice = voice
    }
}

public struct AudioConfigurationV3: Codable, Equatable, Sendable {
    public var schemaVersion: Int
    public var recognition: AudioRecognitionPreference
    public var speech: AudioSpeechPreference
    public var language: String
    public var rate: Double
    public var autoPlayReplies: Bool

    public init(
        schemaVersion: Int = audioConfigurationSchemaVersion,
        recognition: AudioRecognitionPreference,
        speech: AudioSpeechPreference,
        language: String = audioLanguageAuto,
        rate: Double = audioDefaultRate,
        autoPlayReplies: Bool = false
    ) {
        self.schemaVersion = schemaVersion
        self.recognition = recognition
        self.speech = speech
        self.language = language
        self.rate = rate
        self.autoPlayReplies = autoPlayReplies
    }
}

public struct AudioVoiceCatalogEntry: Equatable, Sendable {
    public let source: AudioSource
    public let id: String
    public let modelId: String?
    public let label: String?
    public let aliases: [String]
    public init(source: AudioSource, id: String, modelId: String? = nil, label: String? = nil, aliases: [String] = []) {
        self.source = source
        self.id = id
        self.modelId = modelId
        self.label = label
        self.aliases = aliases
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

    public init<P: AudioRoutePreferenceProviding>(
        kind: AudioProviderKind,
        preference: P,
        language: String,
        systemStatus: AudioReadiness,
        offlineModels: [AudioOfflineModelAvailability],
        systemVoiceIds: [String]? = nil,
        voiceOverride: AudioVoiceSelection? = nil
    ) {
        self.kind = kind
        self.source = preference.source
        self.offlineModelId = preference.offlineModelId
        self.voice = voiceOverride ?? preference.routeVoice
        self.language = language
        self.systemStatus = systemStatus
        self.offlineModels = offlineModels
        self.systemVoiceIds = systemVoiceIds
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
    }
    public let requested: Requested
    public let effective: Effective?
    public let status: AudioRouteStatus
    public let reason: String
    public let fallbackReason: String?
}

public enum AudioConfigurationNormalizer {
    public static var defaults: AudioConfigurationV3 {
        AudioConfigurationV3(
            recognition: AudioRecognitionPreference(source: .automatic),
            speech: AudioSpeechPreference(source: .automatic),
            language: audioLanguageAuto,
            rate: audioDefaultRate,
            autoPlayReplies: false
        )
    }

    public static func normalize(_ value: Any?) -> AudioConfigurationV3 {
        let raw = value as? [String: Any] ?? [:]
        let recognitionRaw = raw["recognition"] as? [String: Any] ?? [:]
        let speechRaw = raw["speech"] as? [String: Any] ?? [:]
        let recognition = AudioRecognitionPreference(
            source: source(recognitionRaw["source"]),
            offlineModelId: modelID(recognitionRaw["offlineModelId"])
        )
        var speech = AudioSpeechPreference(
            source: source(speechRaw["source"]),
            offlineModelId: modelID(speechRaw["offlineModelId"]),
            voice: voice(speechRaw["voice"])
        )
        if speech.source == .automatic, let selectedVoice = speech.voice {
            speech.source = selectedVoice.source
            if selectedVoice.source == .offline, speech.offlineModelId == nil {
                speech.offlineModelId = selectedVoice.modelId
            }
        }
        return AudioConfigurationV3(
            recognition: recognition,
            speech: speech,
            language: language(raw["language"]),
            rate: rate(raw["rate"]),
            autoPlayReplies: (raw["autoPlayReplies"] as? Bool) == true
        )
    }

    public static func migrateLegacy(_ value: Any?, voiceCatalog: [AudioVoiceCatalogEntry] = []) -> AudioConfigurationV3 {
        let raw = value as? [String: Any] ?? [:]
        if (raw["schemaVersion"] as? Int) == audioConfigurationSchemaVersion { return normalize(raw) }
        let recognitionRaw = raw["recognition"] as? [String: Any] ?? [:]
        let speechRaw = raw["speech"] as? [String: Any] ?? [:]
        let recognitionSource = legacySource(recognitionRaw["source"] ?? raw["inputProvider"] ?? raw["recognitionMode"], fallback: .automatic)
        let speechSource = legacySource(speechRaw["source"] ?? raw["outputProvider"] ?? raw["speechProvider"], fallback: .automatic)
        let rawVoice = speechRaw["voice"] ?? raw["voiceSelection"] ?? raw["voiceId"] ?? raw["voice"]
        let selectedVoice: AudioVoiceSelection?
        if let value = rawVoice as? String { selectedVoice = legacyVoice(value, catalog: voiceCatalog) }
        else { selectedVoice = voice(rawVoice) }
        var languageValue = raw["language"] ?? raw["inputLanguage"] ?? raw["legacyInputLanguage"] ?? raw["voiceLanguage"] ?? raw["voiceLang"]
        if languageValue == nil || (languageValue as? String) == "auto" {
            switch (raw["legacyVoiceLang"] as? String)?.lowercased() {
            case "zh": languageValue = "zh-CN"
            case "en": languageValue = "en-US"
            default: break
            }
        }
        var speech = AudioSpeechPreference(
            source: speechSource,
            offlineModelId: modelID(speechRaw["offlineModelId"] ?? raw["speechModelId"] ?? raw["outputModelId"]),
            voice: selectedVoice
        )
        if speech.source == .automatic, let selectedVoice {
            speech.source = selectedVoice.source
            if selectedVoice.source == .offline, speech.offlineModelId == nil { speech.offlineModelId = selectedVoice.modelId }
        }
        return normalize([
            "schemaVersion": audioConfigurationSchemaVersion,
            "recognition": ["source": recognitionSource.rawValue, "offlineModelId": modelID(recognitionRaw["offlineModelId"] ?? raw["recognitionModelId"] ?? raw["inputModelId"]) as Any? ?? NSNull()],
            "speech": ["source": speech.source.rawValue, "offlineModelId": speech.offlineModelId as Any? ?? NSNull(), "voice": voiceJson(speech.voice)],
            "language": languageValue ?? audioLanguageAuto,
            "rate": raw["rate"] ?? raw["speed"] ?? raw["voiceSpeed"] ?? audioDefaultRate,
            "autoPlayReplies": raw["autoPlayReplies"] ?? raw["autoPlay"] ?? raw["voiceAutoPlay"] ?? false
        ] as [String: Any])
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
        if let text = value as? String { return legacyVoice(text, catalog: []) }
        guard let raw = value as? [String: Any], let id = raw["id"] as? String, !id.isEmpty else { return nil }
        let selectedSource = source(raw["source"])
        if selectedSource == .offline {
            return AudioVoiceSelection(source: selectedSource, id: id, modelId: modelID(raw["modelId"]))
        }
        return AudioVoiceSelection(source: selectedSource, id: id)
    }
    private static func legacySource(_ value: Any?, fallback: AudioSource) -> AudioSource {
        guard let textValue = value as? String, !textValue.isEmpty else { return fallback }
        let text = textValue.trimmingCharacters(in: .whitespacesAndNewlines)
        switch text.lowercased() {
        case "automatic", "auto": return .automatic
        case "system": return .system
        case "offline", "localonly", "on-device", "ondevice": return .offline
        default: return AudioSource(rawValue: text)
        }
    }
    private static func uniqueMatch(_ catalog: [AudioVoiceCatalogEntry], selector: String) -> AudioVoiceCatalogEntry? {
        let needle = selector.lowercased()
        let matches = catalog.filter { entry in
            ([entry.id] + [entry.label].compactMap { $0 } + entry.aliases).contains { $0.lowercased() == needle }
        }
        return matches.count == 1 ? matches[0] : nil
    }
    private static func legacyVoice(_ value: String, catalog: [AudioVoiceCatalogEntry]) -> AudioVoiceSelection? {
        let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        if text.hasPrefix("system:") {
            let id = String(text.dropFirst("system:".count))
            let match = uniqueMatch(catalog.filter { $0.source == .system }, selector: id)
            return AudioVoiceSelection(source: .system, id: match?.id ?? id)
        }
        if text.hasPrefix("sherpa:") {
            let payload = String(text.dropFirst("sherpa:".count))
            let parts = payload.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false).map(String.init)
            let modelKey = parts.first ?? payload
            let voiceKey = parts.count > 1 ? parts[1] : payload
            let offline = catalog.filter { $0.source == .offline }
            if let match = uniqueMatch(offline, selector: "\(modelKey):\(voiceKey)"), let modelId = match.modelId {
                return AudioVoiceSelection(source: .offline, id: match.id, modelId: modelId)
            }
            if let match = uniqueMatch(offline, selector: voiceKey), let modelId = match.modelId, modelId == modelKey || modelId.hasSuffix(modelKey) {
                return AudioVoiceSelection(source: .offline, id: match.id, modelId: modelId)
            }
            return AudioVoiceSelection(source: .offline, id: voiceKey, modelId: modelKey)
        }
        if text == "default" { return AudioVoiceSelection(source: .system, id: "default") }
        if let match = uniqueMatch(catalog, selector: text) {
            return AudioVoiceSelection(source: match.source, id: match.id, modelId: match.modelId)
        }
        return AudioVoiceSelection(source: .system, id: text)
    }
    private static func voiceJson(_ value: AudioVoiceSelection?) -> Any {
        guard let value else { return NSNull() }
        var result: [String: Any] = ["source": value.source.rawValue, "id": value.id]
        if let modelId = value.modelId { result["modelId"] = modelId }
        return result
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
