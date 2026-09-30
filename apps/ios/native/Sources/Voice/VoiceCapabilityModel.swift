import AVFoundation
import Observation
import Speech

enum VoiceRecognitionMode: String, CaseIterable, Identifiable {
    case automatic
    case system
    case onDevice = "offline"

    var id: String { rawValue }
    var source: AudioSource {
        switch self {
        case .automatic: .automatic
        case .system: .system
        case .onDevice: .offline
        }
    }

    var title: String {
        switch self {
        case .automatic: String(localized: "voice_mode_automatic_title")
        case .system: "System"
        case .onDevice: String(localized: "voice_mode_on_device_title")
        }
    }

    var detail: String {
        switch self {
        case .automatic: String(localized: "voice_mode_automatic_detail")
        case .system: "Use Apple Speech recognition on this device."
        case .onDevice: String(localized: "voice_mode_on_device_detail")
        }
    }

    init?(source: AudioSource) {
        switch source {
        case .automatic: self = .automatic
        case .system: self = .system
        case .offline: self = .onDevice
        default: return nil
        }
    }
}

enum VoiceSpeechMode: String, CaseIterable, Identifiable {
    case automatic
    case system
    case offline

    var id: String { rawValue }
    var source: AudioSource { AudioSource(rawValue: rawValue) }
    var title: String {
        switch self {
        case .automatic: String(localized: "voice_mode_automatic_title")
        case .system: "System"
        case .offline: String(localized: "voice_mode_on_device_title")
        }
    }

    init?(source: AudioSource) {
        switch source {
        case .automatic: self = .automatic
        case .system: self = .system
        case .offline: self = .offline
        default: return nil
        }
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
    enum Source: Equatable {
        case system
        case sherpa(modelID: String, voiceID: String)
    }

    let id: String
    let name: String
    let language: String
    let quality: AVSpeechSynthesisVoiceQuality
    let source: Source

    init(
        id: String,
        name: String,
        language: String,
        quality: AVSpeechSynthesisVoiceQuality,
        source: Source = .system
    ) {
        self.id = id
        self.name = name
        self.language = language
        self.quality = quality
        self.source = source
    }
}

struct VoiceOfflineModelStatus: Identifiable, Equatable {
    let model: GeneratedOfflineModelEntry
    let state: VoiceModelState
    var id: String { model.id }
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

    private let configurationStore: AudioConfigurationStore
    private let previewPlayback: VoicePreviewPlayback
    private let modelStore: VoiceModelStore

    var language: String
    var mode: VoiceRecognitionMode
    var speechMode: VoiceSpeechMode
    var voiceIdentifier: String
    var speed: Double
    var autoPlay: Bool
    private var unsupportedRecognitionSource: AudioSource?
    private var unsupportedSpeechSource: AudioSource?
    private(set) var speechConfigurationConfirmed = true
    private(set) var ttsConfigurationConfirmed = true
    private(set) var speechAuthorization: SFSpeechRecognizerAuthorizationStatus
    private(set) var microphonePermissionStatus: MicrophonePermissionState
    private(set) var microphoneGranted = false
    private(set) var recognizerAvailable = false
    private(set) var onDeviceAvailable = false
    private(set) var voices: [SystemVoiceOption] = []
    private(set) var modelStates: [String: VoiceModelState] = [:]
    private(set) var isPreviewing = false
    private(set) var errorMessage: String?

    init(
        defaults: UserDefaults = .standard,
        previewPlayback: VoicePreviewPlayback? = nil,
        modelStore: VoiceModelStore? = nil,
        store: AudioConfigurationStore? = nil
    ) {
        let configurationStore = store
            ?? (defaults === UserDefaults.standard ? .shared : AudioConfigurationStore(defaults: defaults))
        self.configurationStore = configurationStore
        self.previewPlayback = previewPlayback ?? .shared
        self.modelStore = modelStore ?? .shared
        let configuration = configurationStore.configuration
        language = configuration.language
        mode = VoiceRecognitionMode(source: configuration.recognition.source) ?? .automatic
        speechMode = VoiceSpeechMode(source: configuration.speech.source) ?? .automatic
        unsupportedRecognitionSource = VoiceRecognitionMode(source: configuration.recognition.source) == nil
            ? configuration.recognition.source
            : nil
        unsupportedSpeechSource = VoiceSpeechMode(source: configuration.speech.source) == nil
            ? configuration.speech.source
            : nil
        voiceIdentifier = Self.legacyVoiceIdentifier(configuration.speech.voice) ?? "auto"
        speed = configuration.rate
        autoPlay = configuration.autoPlayReplies
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

    var configurationRevision: UInt64 { configurationStore.revision }

    var configurationSnapshot: AudioConfigurationSnapshot { configurationStore.snapshot }

    var recognitionRoutePreview: AudioRouteResolution {
        let configuration = configurationStore.configuration
        return resolveAudioRoute(AudioRouteRequest(
            kind: .recognition,
            preference: configuration.recognition,
            language: effectiveLanguageIdentifier,
            systemStatus: recognitionSystemStatus,
            offlineModels: availableOfflineModels
        ))
    }

    var speechRoutePreview: AudioRouteResolution {
        let configuration = configurationStore.configuration
        let selection = Self.audioVoiceSelection(from: voiceIdentifier)
        let preference = AudioSpeechPreference(
            source: unsupportedSpeechSource ?? speechMode.source,
            offlineModelId: configuration.speech.offlineModelId,
            voice: selection
        )
        return resolveAudioRoute(AudioRouteRequest(
            kind: .speech,
            preference: preference,
            language: effectiveLanguageIdentifier,
            systemStatus: systemVoiceIDs.isEmpty
                ? (AVSpeechSynthesisVoice(language: effectiveLanguageIdentifier) == nil ? .unavailable : .available)
                : .available,
            offlineModels: availableOfflineModels,
            systemVoiceIds: systemVoiceIDs
        ))
    }

    private var recognitionSystemStatus: AudioReadiness {
        guard recognizerAvailable else { return .unavailable }
        switch speechAuthorization {
        case .authorized: return .available
        case .notDetermined: return .permissionRequired
        case .denied, .restricted: return .denied
        @unknown default: return .unavailable
        }
    }

    private var systemVoiceIDs: [String] {
        AVSpeechSynthesisVoice.speechVoices().map(\.identifier)
    }

    private var availableOfflineModels: [AudioOfflineModelAvailability] {
        GeneratedVoiceModelCatalog.all.compactMap { model in
            switch model.kind {
            case .stt:
                AudioOfflineModelAvailability(
                    id: model.id,
                    kind: .recognition,
                    languages: model.languages,
                    installed: modelStore.state(for: model.id).isReady
                )
            case .tts:
                AudioOfflineModelAvailability(
                    id: model.id,
                    kind: .speech,
                    languages: model.languages,
                    installed: modelStore.state(for: model.id).isReady,
                    voiceIds: model.voices.map(\.id)
                )
            default:
                nil
            }
        }
    }

    var effectiveRecognitionStatus: VoiceEffectiveRecognitionStatus {
        let resolution = recognitionRoutePreview
        let languageIdentifier = effectiveLanguageIdentifier
        guard let effective = resolution.effective else {
            return VoiceEffectiveRecognitionStatus(
                effectiveLanguageIdentifier: languageIdentifier,
                effectiveLanguageLabel: Self.displayName(for: languageIdentifier),
                modeLabel: String(localized: "voice_temporarily_unavailable"),
                detail: resolution.reason,
                fallbackReason: resolution.fallbackReason ?? resolution.reason
            )
        }
        switch effective.source {
        case .system:
            return VoiceEffectiveRecognitionStatus(
                effectiveLanguageIdentifier: languageIdentifier,
                effectiveLanguageLabel: Self.displayName(for: languageIdentifier),
                modeLabel: mode.title,
                detail: resolution.status == .permissionRequired
                    ? speechPermission.detail
                    : String(localized: "voice_automatic_uses_online"),
                fallbackReason: resolution.fallbackReason
            )
        case .offline:
            let modelID = effective.modelId ?? ""
            let modelName = GeneratedVoiceModelCatalog.byID(modelID)?.displayName["en"] ?? modelID
            return VoiceEffectiveRecognitionStatus(
                effectiveLanguageIdentifier: languageIdentifier,
                effectiveLanguageLabel: Self.displayName(for: languageIdentifier),
                modeLabel: String(localized: "voice_on_device_recognition_title"),
                detail: "Sherpa · \(modelName) · on-device",
                fallbackReason: resolution.fallbackReason
            )
        default:
            return VoiceEffectiveRecognitionStatus(
                effectiveLanguageIdentifier: languageIdentifier,
                effectiveLanguageLabel: Self.displayName(for: languageIdentifier),
                modeLabel: String(localized: "voice_temporarily_unavailable"),
                detail: resolution.reason,
                fallbackReason: resolution.reason
            )
        }
    }

    var effectiveRecognitionLabel: String {
        effectiveRecognitionStatus.detail
    }

    var selectedVoice: SystemVoiceOption? {
        guard let effective = speechRoutePreview.effective else { return nil }
        switch effective.source {
        case .system:
            guard let voiceID = effective.voiceId else {
                return voices.first { $0.id == VoicePreferencesSnapshot.defaultVoiceSelection }
            }
            return voices.first { $0.source == .system && Self.rawVoiceID($0.id) == voiceID }
        case .offline:
            guard let modelID = effective.modelId, let voiceID = effective.voiceId else { return nil }
            return voices.first { $0.source == .sherpa(modelID: modelID, voiceID: voiceID) }
        default:
            return nil
        }
    }

    var configuredVoice: SystemVoiceOption? {
        guard let selection = Self.audioVoiceSelection(from: voiceIdentifier) else { return nil }
        switch selection.source {
        case .system:
            return voices.first { $0.source == .system && Self.rawVoiceID($0.id) == selection.id }
        case .offline:
            guard let modelID = selection.modelId else { return nil }
            return voices.first { $0.source == .sherpa(modelID: modelID, voiceID: selection.id) }
        default:
            return nil
        }
    }

    var requestedVoiceLabel: String {
        if voiceIdentifier == "auto" {
            return String(localized: "voice_mode_automatic_title")
        }
        return configuredVoice.map(Self.voiceLabel(for:)) ?? voiceIdentifier
    }

    var effectiveVoiceLabel: String {
        selectedVoice.map(Self.voiceLabel(for:))
            ?? String(localized: "settings_voice_default_ios")
    }

    var voiceSummaryLabel: String {
        guard let configuredVoice else {
            return voiceIdentifier == "auto"
                ? effectiveVoiceLabel
                : "\(requestedVoiceLabel) → \(effectiveVoiceLabel)"
        }
        guard let selectedVoice, selectedVoice.id != configuredVoice.id else {
            return configuredVoice.name
        }
        return "\(configuredVoice.name) → \(selectedVoice.name)"
    }

    var configurationReadiness: VoiceConfigurationReadiness {
        var issues: [VoiceConfigurationIssue] = []
        if microphonePermissionStatus != .granted {
            issues.append(.init(
                component: .microphone,
                kind: microphonePermissionStatus == .denied ? .permissionDenied : .permissionUndetermined,
                message: microphonePermission.detail
            ))
        }
        let recognition = recognitionRoutePreview
        if recognition.effective?.source == .system,
           speechAuthorization != .authorized {
            issues.append(.init(
                component: .speech,
                kind: speechAuthorization == .denied ? .permissionDenied : .permissionUndetermined,
                message: speechPermission.detail
            ))
        }
        if recognition.effective == nil {
            issues.append(.init(
                component: .speech,
                kind: .unavailable,
                message: recognition.reason
            ))
        }
        let speech = speechRoutePreview
        if speech.status != .ready {
            issues.append(.init(
                component: .tts,
                kind: .unavailable,
                message: speech.reason
            ))
        }
        return VoiceConfigurationReadiness(
            speechConfigured: unsupportedRecognitionSource == nil,
            ttsConfigured: unsupportedSpeechSource == nil,
            speechReady: microphonePermissionStatus == .granted
                && recognition.status == .ready
                && (recognition.effective?.source != .system || speechAuthorization == .authorized),
            ttsReady: speech.status == .ready,
            issues: issues
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
        persistPreferences()
        refreshCapabilities()
    }

    func setMode(_ value: VoiceRecognitionMode) {
        guard mode != value else { return }
        mode = value
        unsupportedRecognitionSource = nil
        persistPreferences()
        refreshCapabilities()
    }

    func setSpeechMode(_ value: VoiceSpeechMode) {
        guard speechMode != value || unsupportedSpeechSource != nil else { return }
        speechMode = value
        unsupportedSpeechSource = nil
        voiceIdentifier = "auto"
        persistPreferences()
        refreshCapabilities()
    }

    func setVoice(_ identifier: String) {
        guard voiceIdentifier != identifier else { return }
        voiceIdentifier = identifier
        if let selection = Self.audioVoiceSelection(from: identifier),
           let selectedMode = VoiceSpeechMode(source: selection.source) {
            speechMode = selectedMode
            unsupportedSpeechSource = nil
        }
        persistPreferences()
        refreshCapabilities()
    }

    func setSpeed(_ value: Double) {
        let clamped = min(2, max(0.5, value))
        guard speed != clamped else { return }
        speed = clamped
        persistPreferences()
    }

    func setAutoPlay(_ value: Bool) {
        guard autoPlay != value else { return }
        autoPlay = value
        persistPreferences()
    }

    /// Re-reads user choices and confirmation markers after returning from a
    /// settings surface, then refreshes permission and hardware availability.
    func reloadFromDefaults() {
        configurationStore.reload()
        load(configurationStore.configuration)
        modelStore.reconcileFromDisk()
        refreshCapabilities()
    }

    /// Saves only device-local audio preferences; operation readiness is
    /// reported separately from the desired settings.
    @discardableResult
    func saveConfiguration() -> VoiceConfigurationReadiness {
        persistPreferences()
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
        modelStates = modelStore.states
        let defaultVoice = SystemVoiceOption(
            id: VoicePreferencesSnapshot.defaultVoiceSelection,
            name: String(localized: "settings_voice_default_ios"),
            language: effectiveLanguageIdentifier,
            quality: .enhanced,
            source: .system
        )
        let automaticVoice = SystemVoiceOption(
            id: "auto",
            name: String(localized: "voice_mode_automatic_title"),
            language: effectiveLanguageIdentifier,
            quality: .default,
            source: .system
        )
        let discoveredSystemVoices = AVSpeechSynthesisVoice.speechVoices()
            .map {
                SystemVoiceOption(
                    id: "system:\($0.identifier)",
                    name: $0.name,
                    language: $0.language,
                    quality: $0.quality,
                    source: .system
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
        let systemVoices = [defaultVoice] + discoveredSystemVoices
        let offlineVoices = GeneratedVoiceModelCatalog.all
            .filter {
                $0.kind == .tts &&
                    $0.languages.contains(languageBase(effectiveLanguageIdentifier)) &&
                    modelStore.state(for: $0.id).isReady
            }
            .flatMap { model in
                model.voices.map { voice in
                    SystemVoiceOption(
                        id: "sherpa:\(model.id):\(voice.id)",
                        name: voice.displayName,
                        language: voice.language,
                        quality: .enhanced,
                        source: .sherpa(modelID: model.id, voiceID: voice.id)
                    )
                }
            }
        voices = [automaticVoice] + systemVoices + offlineVoices
        errorMessage = configurationStore.lastError?.localizedDescription ?? permissionMessage
        IOSAudioService.shared.publishCapabilitySnapshot()
    }

    func requestPermissions() async {
        if mode != .onDevice {
            speechAuthorization = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
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
                speed: speed,
                route: speechRoutePreview,
                maxPayloadBytes: IOSAudioService.shared.maximumPayloadBytes,
                configurationRevision: configurationStore.revision
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

    func downloadOfflinePack() {
        modelStore.downloadPack(languageIdentifier: effectiveLanguageIdentifier)
        syncModelStatesUntilSettled()
    }

    func cancelOfflinePackDownload() {
        for model in GeneratedVoiceModelCatalog.packFor(languageBase(effectiveLanguageIdentifier)) {
            modelStore.cancel(model.id)
        }
        refreshCapabilities()
    }

    var offlinePackStates: [VoiceOfflineModelStatus] {
        GeneratedVoiceModelCatalog.packFor(languageBase(effectiveLanguageIdentifier)).map {
            VoiceOfflineModelStatus(model: $0, state: modelStates[$0.id] ?? .notInstalled)
        }
    }

    private func persistPreferences() {
        let current = configurationStore.configuration
        let voice = Self.audioVoiceSelection(from: voiceIdentifier)
        let speechSource = unsupportedSpeechSource ?? speechMode.source
        let offlineModelID: String?
        if speechSource != .offline {
            offlineModelID = nil
        } else if voice?.source == .offline {
            offlineModelID = voice?.modelId
        } else if current.speech.source == .offline {
            offlineModelID = current.speech.offlineModelId
        } else {
            offlineModelID = nil
        }
        let requested = AudioConfigurationV3(
            recognition: AudioRecognitionPreference(
                source: unsupportedRecognitionSource ?? mode.source,
                offlineModelId: current.recognition.offlineModelId
            ),
            speech: AudioSpeechPreference(
                source: speechSource,
                offlineModelId: offlineModelID,
                voice: voice
            ),
            language: language,
            rate: speed,
            autoPlayReplies: autoPlay
        )
        do {
            _ = try configurationStore.save(requested, expectedRevision: configurationStore.revision)
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
            load(configurationStore.configuration)
        }
    }

    private func load(_ configuration: AudioConfigurationV3) {
        language = configuration.language
        mode = VoiceRecognitionMode(source: configuration.recognition.source) ?? .automatic
        speechMode = VoiceSpeechMode(source: configuration.speech.source) ?? .automatic
        unsupportedRecognitionSource = VoiceRecognitionMode(source: configuration.recognition.source) == nil
            ? configuration.recognition.source
            : nil
        unsupportedSpeechSource = VoiceSpeechMode(source: configuration.speech.source) == nil
            ? configuration.speech.source
            : nil
        voiceIdentifier = Self.legacyVoiceIdentifier(configuration.speech.voice) ?? "auto"
        speed = configuration.rate
        autoPlay = configuration.autoPlayReplies
    }

    private nonisolated static func audioVoiceSelection(from value: String?) -> AudioVoiceSelection? {
        let text = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !text.isEmpty, text != "auto" else { return nil }
        if text.hasPrefix("system:") {
            return AudioVoiceSelection(source: .system, id: String(text.dropFirst("system:".count)))
        }
        if let offline = VoiceRuntimeResolver.parseSherpaVoice(text) {
            return AudioVoiceSelection(source: .offline, id: offline.voiceID, modelId: offline.modelID)
        }
        return AudioVoiceSelection(source: .system, id: text)
    }

    private nonisolated static func legacyVoiceIdentifier(_ selection: AudioVoiceSelection?) -> String? {
        guard let selection else { return nil }
        switch selection.source {
        case .system:
            return "system:\(selection.id)"
        case .offline:
            guard let modelId = selection.modelId else { return "sherpa:unknown:\(selection.id)" }
            return "sherpa:\(modelId):\(selection.id)"
        default:
            return "\(selection.source.rawValue):\(selection.id)"
        }
    }

    private nonisolated static func rawVoiceID(_ optionID: String) -> String {
        optionID.hasPrefix("system:")
            ? String(optionID.dropFirst("system:".count))
            : optionID
    }

    private func syncModelStatesUntilSettled() {
        Task { [weak self] in
            while let self {
                try? await Task.sleep(for: .milliseconds(200))
                self.modelStates = self.modelStore.states
                self.refreshCapabilities()
                let active = self.modelStates.values.contains {
                    switch $0 {
                    case .queued, .downloading, .verifying, .extracting: true
                    default: false
                    }
                }
                if !active { break }
            }
        }
    }

    private nonisolated func languageBase(_ identifier: String) -> String {
        identifier.replacingOccurrences(of: "_", with: "-")
            .split(separator: "-").first.map(String.init)?.lowercased() ?? ""
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
        if microphonePermissionStatus != .granted { return microphonePermission.detail }
        let route = recognitionRoutePreview
        if route.effective?.source == .system, speechAuthorization != .authorized {
            return speechPermission.detail
        }
        return route.fallbackReason ?? (route.effective == nil ? route.reason : nil)
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

    nonisolated static func voiceLabel(for voice: SystemVoiceOption) -> String {
        "\(voice.name) · \(normalizedLocaleIdentifier(voice.language))"
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
        case .system:
            fallbackReason = nil
            modeLabel = String(localized: "voice_mode_automatic_title")
            detail = recognizerAvailable
                ? String(localized: "voice_automatic_uses_online")
                : String(localized: "voice_recognizer_unavailable_retry")
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
