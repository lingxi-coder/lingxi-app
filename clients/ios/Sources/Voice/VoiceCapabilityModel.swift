import AVFoundation
import Observation
import Speech

enum VoiceRecognitionMode: String, CaseIterable, Identifiable {
    case onDevice = "on-device"
    case automatic

    var id: String { rawValue }
    var title: String { self == .onDevice ? String(localized: "voice_mode_on_device_title") : String(localized: "voice_mode_automatic_title") }
    var detail: String {
        self == .onDevice
            ? String(localized: "voice_mode_on_device_detail")
            : String(localized: "voice_mode_automatic_detail")
    }
}

struct VoiceRecognitionLanguageOption: Identifiable, Equatable {
    let id: String
    let title: String
    let detail: String?
}

struct VoiceEffectiveRecognitionStatus: Equatable {
    let effectiveLanguageIdentifier: String
    let effectiveLanguageLabel: String
    let modeLabel: String
    let detail: String
    let fallbackReason: String?
}

struct VoicePermissionDiagnostic: Equatable {
    let label: String
    let detail: String
}

enum MicrophonePermissionState: Equatable {
    case granted
    case denied
    case undetermined
    case unknown
}

struct SystemVoiceOption: Identifiable, Equatable {
    let id: String
    let name: String
    let language: String
    let quality: AVSpeechSynthesisVoiceQuality
}

enum VoiceConfigurationIssueKind: Equatable {
    case unconfigured
    case permissionUndetermined
    case permissionDenied
    case restricted
    case unavailable
}

enum VoiceConfigurationComponent: Equatable {
    case speech
    case microphone
    case tts
}

struct VoiceConfigurationIssue: Equatable {
    let component: VoiceConfigurationComponent
    let kind: VoiceConfigurationIssueKind
    let message: String
}

/// Separates explicit user configuration from permissions and transient system
/// availability so callers can route users to the correct recovery action.
struct VoiceConfigurationReadiness: Equatable {
    let speechConfigured: Bool
    let ttsConfigured: Bool
    let speechReady: Bool
    let ttsReady: Bool
    let issues: [VoiceConfigurationIssue]

    var isReadyForDictation: Bool { speechReady }
    var isReadyForFlow: Bool { speechReady && ttsReady }
    var message: String? { issues.first?.message }
}

/// One source of truth for the native speech capability surfaced by onboarding,
/// settings, and Flow Mode. The engine callbacks read the same persisted keys.
@Observable
@MainActor
final class VoiceCapabilityModel {
    nonisolated static let automaticLanguageIdentifier = "auto"
    nonisolated static let currentSpeechConfigurationVersion = 1
    nonisolated static let currentTTSConfigurationVersion = 1

    private nonisolated static let speechConfigurationVersionKey = "voiceSpeechConfigurationVersion"
    private nonisolated static let ttsConfigurationVersionKey = "voiceTTSConfigurationVersion"

    private let defaults: UserDefaults
    private let previewPlayback: VoicePreviewPlayback

    var language: String
    var mode: VoiceRecognitionMode
    var voiceIdentifier: String
    var speed: Double
    var autoPlay: Bool
    private(set) var speechConfigurationConfirmed: Bool
    private(set) var ttsConfigurationConfirmed: Bool
    private(set) var speechAuthorization: SFSpeechRecognizerAuthorizationStatus
    private(set) var microphonePermissionStatus: MicrophonePermissionState
    private(set) var microphoneGranted = false
    private(set) var recognizerAvailable = false
    private(set) var onDeviceAvailable = false
    private(set) var voices: [SystemVoiceOption] = []
    private(set) var isPreviewing = false
    private(set) var errorMessage: String?

    init(
        defaults: UserDefaults = .standard,
        previewPlayback: VoicePreviewPlayback? = nil
    ) {
        self.defaults = defaults
        self.previewPlayback = previewPlayback ?? .shared
        language = defaults.string(forKey: "voiceLanguage") ?? Self.automaticLanguageIdentifier
        mode = VoiceRecognitionMode(
            rawValue: defaults.string(forKey: "voiceRecognitionMode") ?? "on-device"
        ) ?? .onDevice
        voiceIdentifier = defaults.string(forKey: "systemVoiceIdentifier") ?? ""
        speed = defaults.object(forKey: "voiceSpeed") == nil ? 1 : defaults.double(forKey: "voiceSpeed")
        autoPlay = defaults.bool(forKey: "voiceAutoPlay")
        speechConfigurationConfirmed = defaults.integer(forKey: Self.speechConfigurationVersionKey)
            == Self.currentSpeechConfigurationVersion
        ttsConfigurationConfirmed = defaults.integer(forKey: Self.ttsConfigurationVersionKey)
            == Self.currentTTSConfigurationVersion
        speechAuthorization = SFSpeechRecognizer.authorizationStatus()
        microphonePermissionStatus = Self.resolveMicrophonePermissionStatus()
        microphoneGranted = microphonePermissionStatus == .granted
        refreshCapabilities()
    }

    var languageOptions: [VoiceRecognitionLanguageOption] {
        [
            .init(
                id: Self.automaticLanguageIdentifier,
                title: String(localized: "onboarding_voice_language_system"),
                detail: String(localized: "voice_lang_option_current \(effectiveLanguageLabel)")
            ),
            .init(id: "zh-CN", title: String(localized: "common_lang_zh_hans"), detail: nil),
            .init(id: "en-US", title: "English (US)", detail: nil),
            .init(id: "ja-JP", title: String(localized: "onboarding_voice_language_ja"), detail: nil),
        ]
    }

    var selectedLanguageLabel: String {
        if language == Self.automaticLanguageIdentifier {
            return String(localized: "onboarding_voice_language_system")
        }
        return Self.displayName(for: language)
    }

    var effectiveLanguageIdentifier: String {
        Self.resolvedRecognitionLocaleIdentifier(
            configuredLanguage: language,
            currentLocale: .autoupdatingCurrent
        )
    }

    var effectiveLanguageLabel: String {
        Self.displayName(for: effectiveLanguageIdentifier)
    }

    var effectiveRecognitionStatus: VoiceEffectiveRecognitionStatus {
        Self.buildEffectiveRecognitionStatus(
            mode: mode,
            requestedLanguage: language,
            currentLocale: .autoupdatingCurrent,
            recognizerAvailable: recognizerAvailable,
            onDeviceAvailable: onDeviceAvailable,
            speechAuthorization: speechAuthorization,
            microphoneGranted: microphoneGranted
        )
    }

    var effectiveRecognitionLabel: String {
        effectiveRecognitionStatus.detail
    }

    var selectedVoice: SystemVoiceOption? {
        voices.first { $0.id == voiceIdentifier }
            ?? voices.first { Self.normalizedLocaleIdentifier($0.language) == effectiveLanguageIdentifier }
            ?? voices.first
    }

    var configuredVoice: SystemVoiceOption? {
        guard !voiceIdentifier.isEmpty else { return nil }
        return voices.first { $0.id == voiceIdentifier }
    }

    var configurationReadiness: VoiceConfigurationReadiness {
        Self.buildConfigurationReadiness(
            speechConfigured: speechConfigurationConfirmed,
            ttsConfigured: ttsConfigurationConfirmed,
            speechAuthorization: speechAuthorization,
            microphonePermissionStatus: microphonePermissionStatus,
            recognizerAvailable: recognizerAvailable,
            hasConfiguredVoice: configuredVoice != nil,
            systemVoicesAvailable: !voices.isEmpty
        )
    }

    /// Compatibility shorthand for call sites that only need the current
    /// aggregate state.
    var readiness: VoiceConfigurationReadiness { configurationReadiness }

    var speechPermission: VoicePermissionDiagnostic {
        Self.speechPermissionDiagnostic(speechAuthorization)
    }

    var microphonePermission: VoicePermissionDiagnostic {
        Self.microphonePermissionDiagnostic(microphonePermissionStatus)
    }

    func setLanguage(_ value: String) {
        guard language != value else { return }
        language = value
        defaults.set(value, forKey: "voiceLanguage")
        defaults.removeObject(forKey: Self.speechConfigurationVersionKey)
        speechConfigurationConfirmed = false
        refreshCapabilities()
    }

    func setMode(_ value: VoiceRecognitionMode) {
        guard mode != value else { return }
        mode = value
        defaults.set(value.rawValue, forKey: "voiceRecognitionMode")
        defaults.removeObject(forKey: Self.speechConfigurationVersionKey)
        speechConfigurationConfirmed = false
    }

    func setVoice(_ identifier: String) {
        guard voiceIdentifier != identifier else { return }
        voiceIdentifier = identifier
        defaults.set(identifier, forKey: "systemVoiceIdentifier")
        defaults.removeObject(forKey: Self.ttsConfigurationVersionKey)
        ttsConfigurationConfirmed = false
    }

    func setSpeed(_ value: Double) {
        let clamped = min(2, max(0.5, value))
        guard speed != clamped else { return }
        speed = clamped
        defaults.set(clamped, forKey: "voiceSpeed")
    }

    func setAutoPlay(_ value: Bool) {
        guard autoPlay != value else { return }
        autoPlay = value
        defaults.set(value, forKey: "voiceAutoPlay")
    }

    /// Re-reads user choices and confirmation markers after returning from a
    /// settings surface, then refreshes permission and hardware availability.
    func reloadFromDefaults() {
        language = defaults.string(forKey: "voiceLanguage") ?? Self.automaticLanguageIdentifier
        mode = VoiceRecognitionMode(
            rawValue: defaults.string(forKey: "voiceRecognitionMode") ?? VoiceRecognitionMode.onDevice.rawValue
        ) ?? .onDevice
        voiceIdentifier = defaults.string(forKey: "systemVoiceIdentifier") ?? ""
        speed = defaults.object(forKey: "voiceSpeed") == nil ? 1 : defaults.double(forKey: "voiceSpeed")
        autoPlay = defaults.bool(forKey: "voiceAutoPlay")
        speechConfigurationConfirmed = defaults.integer(forKey: Self.speechConfigurationVersionKey)
            == Self.currentSpeechConfigurationVersion
        ttsConfigurationConfirmed = defaults.integer(forKey: Self.ttsConfigurationVersionKey)
            == Self.currentTTSConfigurationVersion
        refreshCapabilities()
    }

    /// Persists default-valued choices as explicit user decisions. TTS is only
    /// confirmed when a concrete system voice can be persisted.
    @discardableResult
    func saveConfiguration() -> VoiceConfigurationReadiness {
        defaults.set(language, forKey: "voiceLanguage")
        defaults.set(mode.rawValue, forKey: "voiceRecognitionMode")
        defaults.set(Self.currentSpeechConfigurationVersion, forKey: Self.speechConfigurationVersionKey)
        speechConfigurationConfirmed = true

        if let voice = configuredVoice ?? selectedVoice {
            voiceIdentifier = voice.id
            defaults.set(voice.id, forKey: "systemVoiceIdentifier")
            defaults.set(Self.currentTTSConfigurationVersion, forKey: Self.ttsConfigurationVersionKey)
            ttsConfigurationConfirmed = true
        } else {
            defaults.removeObject(forKey: Self.ttsConfigurationVersionKey)
            ttsConfigurationConfirmed = false
        }

        return configurationReadiness
    }

    func refreshCapabilities() {
        speechAuthorization = SFSpeechRecognizer.authorizationStatus()
        microphonePermissionStatus = Self.resolveMicrophonePermissionStatus()
        microphoneGranted = microphonePermissionStatus == .granted
        let recognizer = SFSpeechRecognizer(locale: Locale(identifier: effectiveLanguageIdentifier))
        recognizerAvailable = recognizer?.isAvailable == true
        onDeviceAvailable = recognizer?.supportsOnDeviceRecognition == true
        let effectiveLanguageIdentifier = effectiveLanguageIdentifier
        voices = AVSpeechSynthesisVoice.speechVoices()
            .map {
                SystemVoiceOption(
                    id: $0.identifier,
                    name: $0.name,
                    language: $0.language,
                    quality: $0.quality
                )
            }
            .sorted { lhs, rhs in
                let lhsMatches = Self.normalizedLocaleIdentifier(lhs.language) == effectiveLanguageIdentifier
                let rhsMatches = Self.normalizedLocaleIdentifier(rhs.language) == effectiveLanguageIdentifier
                if lhsMatches != rhsMatches {
                    return lhsMatches
                }
                if lhs.quality != rhs.quality { return lhs.quality.rawValue > rhs.quality.rawValue }
                return lhs.name.localizedStandardCompare(rhs.name) == .orderedAscending
            }
        errorMessage = permissionMessage
    }

    func requestPermissions() async {
        speechAuthorization = await withCheckedContinuation { continuation in
            SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
        }
        microphoneGranted = await AVAudioApplication.requestRecordPermission()
        microphonePermissionStatus = microphoneGranted ? .granted : Self.resolveMicrophonePermissionStatus()
        refreshCapabilities()
        errorMessage = permissionMessage
    }

    func preview(_ text: String = String(localized: "voice_preview_default_text")) async {
        guard !isPreviewing else { return }
        isPreviewing = true
        errorMessage = nil
        defer { isPreviewing = false }
        do {
            guard let voice = selectedVoice else {
                errorMessage = String(localized: "voice_no_playback_voice_available")
                return
            }
            let request = VoiceSpeechRequest(
                text: text,
                voiceIdentifier: voice.id,
                languageIdentifier: effectiveLanguageIdentifier,
                speed: speed
            )
            let outcome = try await previewPlayback.play(request)
            if outcome == .interrupted {
                errorMessage = String(localized: "voice_preview_interrupted")
            }
        } catch is CancellationError {
            return
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func stopPreview() async {
        await previewPlayback.stop()
        isPreviewing = false
    }

    nonisolated static func configurationSaveMessage(
        for readiness: VoiceConfigurationReadiness
    ) -> String {
        if readiness.isReadyForFlow {
            return String(localized: "voice_config_saved_ready")
        }
        if let issue = readiness.issues.first {
            return String(localized: "voice_config_saved_with_issue \(issue.message)")
        }
        if readiness.speechConfigured {
            return String(localized: "voice_config_saved_no_voice")
        }
        return String(localized: "voice_config_not_finished")
    }

    private var permissionMessage: String? {
        if speechAuthorization != .authorized {
            return speechPermission.detail
        }
        if microphonePermissionStatus != .granted { return microphonePermission.detail }
        if let fallbackReason = effectiveRecognitionStatus.fallbackReason,
           mode == .onDevice {
            return fallbackReason
        }
        return nil
    }

    nonisolated static func normalizedLocaleIdentifier(_ identifier: String) -> String {
        identifier
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "_", with: "-")
    }

    nonisolated static func resolvedRecognitionLocaleIdentifier(
        configuredLanguage: String?,
        currentLocale: Locale = .autoupdatingCurrent
    ) -> String {
        let normalizedConfigured = configuredLanguage.map(normalizedLocaleIdentifier)
        if normalizedConfigured == nil || normalizedConfigured == automaticLanguageIdentifier {
            let current = normalizedLocaleIdentifier(currentLocale.identifier)
            return current.isEmpty ? "zh-CN" : current
        }
        return normalizedConfigured ?? "zh-CN"
    }

    nonisolated static func displayName(for identifier: String, currentLocale: Locale = .autoupdatingCurrent) -> String {
        let normalized = normalizedLocaleIdentifier(identifier)
        if normalized == automaticLanguageIdentifier {
            return String(localized: "onboarding_voice_language_system")
        }
        let locale = Locale(identifier: normalized)
        let localized = currentLocale.localizedString(forIdentifier: normalized)
            ?? currentLocale.localizedString(forIdentifier: locale.identifier)
            ?? normalized
        return "\(localized) · \(normalized)"
    }

    nonisolated static func buildEffectiveRecognitionStatus(
        mode: VoiceRecognitionMode,
        requestedLanguage: String,
        currentLocale: Locale,
        recognizerAvailable: Bool,
        onDeviceAvailable: Bool,
        speechAuthorization: SFSpeechRecognizerAuthorizationStatus,
        microphoneGranted: Bool
    ) -> VoiceEffectiveRecognitionStatus {
        let effectiveLanguageIdentifier = resolvedRecognitionLocaleIdentifier(
            configuredLanguage: requestedLanguage,
            currentLocale: currentLocale
        )
        let effectiveLanguageLabel = displayName(for: effectiveLanguageIdentifier, currentLocale: currentLocale)
        let permissionBlocked = speechAuthorization != .authorized || !microphoneGranted
        let fallbackReason: String?
        let modeLabel: String
        let detail: String

        switch mode {
        case .automatic:
            fallbackReason = nil
            modeLabel = String(localized: "voice_mode_automatic_title")
            if !recognizerAvailable {
                detail = String(localized: "voice_recognizer_unavailable_request_may_fail")
            } else if onDeviceAvailable {
                detail = String(localized: "voice_automatic_supports_on_device")
            } else {
                detail = String(localized: "voice_automatic_uses_online")
            }
        case .onDevice:
            if permissionBlocked {
                fallbackReason = String(localized: "voice_permission_not_met")
                modeLabel = String(localized: "voice_waiting_permission")
                detail = String(localized: "voice_needs_permission_setup")
            } else if !recognizerAvailable {
                fallbackReason = String(localized: "voice_recognizer_unavailable_for_language")
                modeLabel = String(localized: "voice_temporarily_unavailable")
                detail = String(localized: "voice_recognizer_unavailable_retry")
            } else if onDeviceAvailable {
                fallbackReason = nil
                modeLabel = String(localized: "voice_on_device_recognition_title")
                detail = String(localized: "voice_on_device_supported_detail")
            } else {
                fallbackReason = String(localized: "voice_on_device_not_supported")
                modeLabel = String(localized: "voice_online_fallback_title")
                detail = String(localized: "voice_online_fallback_detail")
            }
        }

        return VoiceEffectiveRecognitionStatus(
            effectiveLanguageIdentifier: effectiveLanguageIdentifier,
            effectiveLanguageLabel: effectiveLanguageLabel,
            modeLabel: modeLabel,
            detail: detail,
            fallbackReason: fallbackReason
        )
    }

    nonisolated static func speechPermissionDiagnostic(
        _ status: SFSpeechRecognizerAuthorizationStatus
    ) -> VoicePermissionDiagnostic {
        switch status {
        case .authorized:
            return .init(label: String(localized: "voice_permission_authorized_label"), detail: String(localized: "voice_speech_permission_ok"))
        case .denied:
            return .init(label: String(localized: "voice_permission_denied_label"), detail: String(localized: "voice_speech_permission_denied_detail"))
        case .restricted:
            return .init(label: String(localized: "voice_permission_restricted_label"), detail: String(localized: "voice_speech_restricted_detail"))
        case .notDetermined:
            return .init(label: String(localized: "voice_permission_pending_label"), detail: String(localized: "voice_speech_permission_first_use"))
        @unknown default:
            return .init(label: String(localized: "voice_permission_unknown_label"), detail: String(localized: "voice_speech_permission_unknown_detail"))
        }
    }

    nonisolated static func microphonePermissionDiagnostic(_ granted: Bool) -> VoicePermissionDiagnostic {
        granted
            ? .init(label: String(localized: "voice_permission_authorized_label"), detail: String(localized: "voice_mic_permission_ok"))
            : .init(label: String(localized: "voice_permission_pending_label"), detail: String(localized: "voice_mic_permission_allow_hint"))
    }

    nonisolated static func microphonePermissionDiagnostic(
        _ status: MicrophonePermissionState
    ) -> VoicePermissionDiagnostic {
        switch status {
        case .granted:
            return .init(label: String(localized: "voice_permission_authorized_label"), detail: String(localized: "voice_mic_permission_ok"))
        case .denied:
            return .init(label: String(localized: "voice_permission_denied_label"), detail: String(localized: "voice_mic_permission_enable_hint"))
        case .undetermined:
            return .init(label: String(localized: "voice_permission_pending_label"), detail: String(localized: "voice_mic_permission_first_use"))
        case .unknown:
            return .init(label: String(localized: "voice_permission_unknown_label"), detail: String(localized: "voice_mic_permission_unknown_detail"))
        }
    }

    nonisolated static func resolveMicrophonePermissionStatus() -> MicrophonePermissionState {
        switch AVAudioApplication.shared.recordPermission {
        case .granted:
            return .granted
        case .denied:
            return .denied
        case .undetermined:
            return .undetermined
        @unknown default:
            return .unknown
        }
    }

    nonisolated static func utteranceRate(from multiplier: Double) -> Float {
        let normalized = min(2, max(0.5, multiplier))
        return AVSpeechUtteranceDefaultSpeechRate * Float(normalized)
    }

    nonisolated static func buildConfigurationReadiness(
        speechConfigured: Bool,
        ttsConfigured: Bool,
        speechAuthorization: SFSpeechRecognizerAuthorizationStatus,
        microphonePermissionStatus: MicrophonePermissionState,
        recognizerAvailable: Bool,
        hasConfiguredVoice: Bool,
        systemVoicesAvailable: Bool
    ) -> VoiceConfigurationReadiness {
        var issues: [VoiceConfigurationIssue] = []

        if !speechConfigured {
            issues.append(.init(
                component: .speech,
                kind: .unconfigured,
                message: String(localized: "voice_issue_save_language_mode")
            ))
        }

        switch speechAuthorization {
        case .authorized:
            break
        case .notDetermined:
            issues.append(.init(
                component: .speech,
                kind: .permissionUndetermined,
                message: String(localized: "voice_issue_need_speech_permission")
            ))
        case .denied:
            issues.append(.init(
                component: .speech,
                kind: .permissionDenied,
                message: String(localized: "voice_issue_speech_permission_off")
            ))
        case .restricted:
            issues.append(.init(
                component: .speech,
                kind: .restricted,
                message: String(localized: "voice_issue_device_restricted")
            ))
        @unknown default:
            issues.append(.init(
                component: .speech,
                kind: .unavailable,
                message: String(localized: "voice_issue_speech_permission_unknown")
            ))
        }

        switch microphonePermissionStatus {
        case .granted:
            break
        case .undetermined:
            issues.append(.init(
                component: .microphone,
                kind: .permissionUndetermined,
                message: String(localized: "voice_issue_need_mic_permission")
            ))
        case .denied:
            issues.append(.init(
                component: .microphone,
                kind: .permissionDenied,
                message: String(localized: "voice_issue_mic_permission_off")
            ))
        case .unknown:
            issues.append(.init(
                component: .microphone,
                kind: .unavailable,
                message: String(localized: "voice_issue_mic_permission_unknown")
            ))
        }

        if !recognizerAvailable {
            issues.append(.init(
                component: .speech,
                kind: .unavailable,
                message: String(localized: "voice_recognizer_unavailable_for_language")
            ))
        }

        if !ttsConfigured {
            issues.append(.init(
                component: .tts,
                kind: .unconfigured,
                message: String(localized: "voice_issue_save_voice")
            ))
        } else if !systemVoicesAvailable {
            issues.append(.init(
                component: .tts,
                kind: .unavailable,
                message: String(localized: "voice_no_playback_voice_available")
            ))
        } else if !hasConfiguredVoice {
            issues.append(.init(
                component: .tts,
                kind: .unavailable,
                message: String(localized: "voice_issue_voice_not_available")
            ))
        }

        let speechReady = speechConfigured
            && speechAuthorization == .authorized
            && microphonePermissionStatus == .granted
            && recognizerAvailable
        let ttsReady = ttsConfigured && systemVoicesAvailable && hasConfiguredVoice

        return VoiceConfigurationReadiness(
            speechConfigured: speechConfigured,
            ttsConfigured: ttsConfigured,
            speechReady: speechReady,
            ttsReady: ttsReady,
            issues: issues
        )
    }
}
