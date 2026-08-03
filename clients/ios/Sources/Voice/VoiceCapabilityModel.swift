import AVFoundation
import Observation
import Speech

enum VoiceRecognitionMode: String, CaseIterable, Identifiable {
    case onDevice = "on-device"
    case automatic

    var id: String { rawValue }
    var title: String { self == .onDevice ? "优先设备端" : "系统自动" }
    var detail: String {
        self == .onDevice
            ? "支持时强制在设备上识别；不支持的语言会明确回退。"
            : "允许系统根据设备、语言和网络选择识别方式。"
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
                title: "跟随系统",
                detail: "当前：\(effectiveLanguageLabel)"
            ),
            .init(id: "zh-CN", title: "简体中文", detail: nil),
            .init(id: "en-US", title: "English (US)", detail: nil),
            .init(id: "ja-JP", title: "日本語", detail: nil),
        ]
    }

    var selectedLanguageLabel: String {
        if language == Self.automaticLanguageIdentifier {
            return "跟随系统"
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

    func preview(_ text: String = "你好，我是灵犀。语音设置已经生效。") async {
        guard !isPreviewing else { return }
        isPreviewing = true
        errorMessage = nil
        defer { isPreviewing = false }
        do {
            guard let voice = selectedVoice else {
                errorMessage = "系统当前没有可用的播报声音"
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
                errorMessage = "试听被系统音频中断，请重试"
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
            return "语音识别和系统播报声音已保存并可用。"
        }
        if let issue = readiness.issues.first {
            return "语音配置已保存；\(issue.message)"
        }
        if readiness.speechConfigured {
            return "语音识别已保存，但当前没有可用的系统播报声音。"
        }
        return "语音配置尚未完成。"
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
            return "跟随系统"
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
            modeLabel = "系统自动"
            if !recognizerAvailable {
                detail = "当前语言识别器暂不可用，系统请求可能失败"
            } else if onDeviceAvailable {
                detail = "系统可按条件选择设备端或在线识别；当前语言支持设备端"
            } else {
                detail = "系统将自动选择识别方式；当前语言通常会使用在线识别"
            }
        case .onDevice:
            if permissionBlocked {
                fallbackReason = "权限未满足，无法开始语音识别"
                modeLabel = "等待权限"
                detail = "需要先完成语音识别与麦克风授权"
            } else if !recognizerAvailable {
                fallbackReason = "当前系统语言的识别器暂不可用"
                modeLabel = "暂不可用"
                detail = "系统识别器当前不可用，请稍后重试或切换语言"
            } else if onDeviceAvailable {
                fallbackReason = nil
                modeLabel = "设备端识别"
                detail = "当前语言支持设备端识别，将优先在本机完成"
            } else {
                fallbackReason = "当前语言不支持设备端识别"
                modeLabel = "系统在线回退"
                detail = "设备端不可用，将回退到系统在线识别"
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
            return .init(label: "已授权", detail: "语音识别权限正常")
        case .denied:
            return .init(label: "已拒绝", detail: "请在系统设置中开启“语音识别”权限")
        case .restricted:
            return .init(label: "受限制", detail: "设备或家长控制限制了语音识别")
        case .notDetermined:
            return .init(label: "待授权", detail: "首次使用时会请求语音识别权限")
        @unknown default:
            return .init(label: "未知", detail: "系统未返回明确的语音识别权限状态")
        }
    }

    nonisolated static func microphonePermissionDiagnostic(_ granted: Bool) -> VoicePermissionDiagnostic {
        granted
            ? .init(label: "已授权", detail: "麦克风权限正常")
            : .init(label: "待授权", detail: "请在系统设置中允许麦克风访问")
    }

    nonisolated static func microphonePermissionDiagnostic(
        _ status: MicrophonePermissionState
    ) -> VoicePermissionDiagnostic {
        switch status {
        case .granted:
            return .init(label: "已授权", detail: "麦克风权限正常")
        case .denied:
            return .init(label: "已拒绝", detail: "请在系统设置中开启麦克风权限")
        case .undetermined:
            return .init(label: "待授权", detail: "首次使用时会请求麦克风权限")
        case .unknown:
            return .init(label: "未知", detail: "系统未返回明确的麦克风权限状态")
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
                message: "请先保存语音识别语言和识别方式"
            ))
        }

        switch speechAuthorization {
        case .authorized:
            break
        case .notDetermined:
            issues.append(.init(
                component: .speech,
                kind: .permissionUndetermined,
                message: "需要授予语音识别权限"
            ))
        case .denied:
            issues.append(.init(
                component: .speech,
                kind: .permissionDenied,
                message: "语音识别权限已关闭，请前往系统设置开启"
            ))
        case .restricted:
            issues.append(.init(
                component: .speech,
                kind: .restricted,
                message: "当前设备限制了语音识别"
            ))
        @unknown default:
            issues.append(.init(
                component: .speech,
                kind: .unavailable,
                message: "无法确认语音识别权限状态"
            ))
        }

        switch microphonePermissionStatus {
        case .granted:
            break
        case .undetermined:
            issues.append(.init(
                component: .microphone,
                kind: .permissionUndetermined,
                message: "需要授予麦克风权限"
            ))
        case .denied:
            issues.append(.init(
                component: .microphone,
                kind: .permissionDenied,
                message: "麦克风权限已关闭，请前往系统设置开启"
            ))
        case .unknown:
            issues.append(.init(
                component: .microphone,
                kind: .unavailable,
                message: "无法确认麦克风权限状态"
            ))
        }

        if !recognizerAvailable {
            issues.append(.init(
                component: .speech,
                kind: .unavailable,
                message: "当前语言的系统识别器暂不可用"
            ))
        }

        if !ttsConfigured {
            issues.append(.init(
                component: .tts,
                kind: .unconfigured,
                message: "请先选择并保存系统播报声音"
            ))
        } else if !systemVoicesAvailable {
            issues.append(.init(
                component: .tts,
                kind: .unavailable,
                message: "系统当前没有可用的播报声音"
            ))
        } else if !hasConfiguredVoice {
            issues.append(.init(
                component: .tts,
                kind: .unavailable,
                message: "已配置的系统声音不可用，请重新选择"
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
