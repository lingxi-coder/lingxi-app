import SwiftUI

// MARK: - First-run setup wizard
//
// Port of the prototype's `SetupWizard` (lingxi-iphone.html). A 6-step first-run
// flow over the sci-fi orb backdrop: welcome → name the assistant → configure
// a Provider credential and model → review web capabilities → choose native
// speech behavior → finish.
// On finish it writes the chosen values to `AppState` and marks `setupDone`, so
// it only runs once (re-triggerable from Settings → 关于 → 重新观看引导).

struct SetupWizardView: View {
    @Environment(AppState.self) private var app
    @Environment(VoiceCapabilityModel.self) private var voiceCapability
    @Bindable var store: SettingsStore
    /// Open the full settings flow without dismissing onboarding.
    var onOpenSettings: (SettingsPage) -> Void = { _ in }
    /// Called once the wizard commits (or is finished) — RootView dismisses it
    /// by reading `app.setupDone`; this is the hook for any extra teardown.
    var onDone: () -> Void = {}

    private static let total = 6

    @State private var step = 0
    @State private var seeded = false
    @State private var providerRepository = ProviderRepository.shared
    @State private var editingProviderDraft: ProviderEditorDraft?

    // Editable copy, seeded from the current app state on first appear.
    @State private var assistantName = String(localized: "app_name")

    @FocusState private var fieldFocused: Bool

    var body: some View {
        ZStack {
            RadialGradient(colors: [Color(okl: 0.19, 0.07, 275), Color(srgb: 0.020, 0.020, 0.035)],
                           center: UnitPoint(x: 0.5, y: 0.26), startRadius: 0, endRadius: 560)
                .ignoresSafeArea()
            RadialGradient(colors: [.clear, .black.opacity(0.5)],
                           center: UnitPoint(x: 0.5, y: 0.34), startRadius: 140, endRadius: 480)
                .ignoresSafeArea().allowsHitTesting(false)

            VStack(spacing: 0) {
                header
                GeometryReader { viewport in
                    ScrollView {
                        bodyContent
                            .padding(.horizontal, 30)
                            .padding(.vertical, 16)
                            .frame(
                                maxWidth: .infinity,
                                minHeight: viewport.size.height,
                                alignment: .center
                            )
                            .id(step)
                            .transition(.asymmetric(insertion: .move(edge: .trailing).combined(with: .opacity),
                                                    removal: .opacity))
                    }
                    .scrollDismissesKeyboard(.interactively)
                    .accessibilityIdentifier("onboarding.content")
                }
                footer
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        .onAppear {
            guard !seeded else { return }
            seeded = true
            assistantName = app.assistantName
            voiceCapability.reloadFromDefaults()
        }
        .onChange(of: step) { oldStep, newStep in
            guard oldStep == 4, newStep != 4 else { return }
            Task { await voiceCapability.stopPreview() }
        }
        .onDisappear {
            Task { await voiceCapability.stopPreview() }
        }
        .sheet(item: $editingProviderDraft) { draft in
            ProviderEditorSheet(draft: draft) {
                editingProviderDraft = nil
            }
            .presentationDragIndicator(.visible)
        }
        .task {
            await providerRepository.refreshCatalog()
            await providerRepository.refreshCredentialStatus()
        }
    }

    // MARK: header — back chevron + segmented progress
    private var header: some View {
        HStack(spacing: 12) {
            if step > 0 {
                Button { withAnimation(.easeOut(duration: 0.3)) { step = max(0, step - 1) } } label: {
                    LXIcon(name: .chevronR, size: 17, color: Color(okl: 0.88, 0.02, 275), stroke: 2)
                        .scaleEffect(x: -1, y: 1)
                        .frame(width: 36, height: 36)
                        .background(.white.opacity(0.06), in: Circle())
                        .overlay(Circle().stroke(.white.opacity(0.12), lineWidth: 0.5))
                }
                // The label already draws its own circle; without `.plain` the
                // system default style paints a second, rounded-rect plate
                // behind it. Every other button in this app that supplies a
                // background declares `.plain` — these two were the omissions.
                .buttonStyle(.plain)
                .accessibilityLabel("onboarding_back")
            } else {
                Color.clear.frame(width: 36, height: 36)
            }
            HStack(spacing: 5) {
                ForEach(0..<Self.total, id: \.self) { k in
                    Capsule()
                        .fill(k <= step
                              ? AnyShapeStyle(LinearGradient(colors: [Color(okl: 0.72, 0.18, 268), Color(okl: 0.70, 0.18, 318)], startPoint: .leading, endPoint: .trailing))
                              : AnyShapeStyle(Color.white.opacity(0.12)))
                        .frame(height: 3)
                }
            }
            // Symmetric twin of the leading 36×36 slot. Width-only made this
            // an UNBOUNDED-height Color: the header became a flexible VStack
            // child, grabbed ~half the leftover vertical space (~300pt), and
            // the progress bar rendered centered a third of the way down the
            // screen on every step.
            Color.clear.frame(width: 36, height: 36)
        }
        .padding(.horizontal, 18)
        .padding(.top, 8)
        .frame(minHeight: 40)
        .accessibilityIdentifier("onboarding.header")
    }

    // MARK: footer — primary CTA + optional skip
    private var footer: some View {
        VStack(spacing: 12) {
            Button(action: next) {
                HStack(spacing: 8) {
                    Text(cta).font(.system(size: 16, weight: .semibold)).kerning(0.5)
                    LXIcon(name: step == Self.total - 1 ? .sparkle : .arrowRight, size: 17,
                           color: ctaDisabled ? Color(okl: 0.60, 0.02, 280) : .white, stroke: 2)
                }
                .foregroundColor(ctaDisabled ? Color(okl: 0.60, 0.02, 280) : .white)
                .frame(maxWidth: .infinity).frame(height: 54)
                .background {
                    if ctaDisabled { Color.white.opacity(0.1) }
                    else { LinearGradient(colors: [Color(okl: 0.70, 0.19, 270), Color(okl: 0.66, 0.20, 305)], startPoint: .topLeading, endPoint: .bottomTrailing) }
                }
                .clipShape(RoundedRectangle(cornerRadius: 16))
                .shadow(color: ctaDisabled ? .clear : Color(okl: 0.66, 0.20, 290, 0.4), radius: 12, y: 8)
            }
            // Same omission as the back chevron: the gradient/opacity fill and
            // the 16pt clip are the button's own, so the default style's plate
            // shows through around them. `.plain` also drops the automatic
            // disabled dimming, which is already expressed explicitly above.
            .buttonStyle(.plain)
            .disabled(ctaDisabled)
            .accessibilityIdentifier("onboarding.primaryAction")

            if let skip = skipLabel {
                Button(skip) { withAnimation(.easeOut(duration: 0.3)) { step += 1 } }
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundColor(Color(okl: 0.60, 0.03, 275))
            }
        }
        .padding(.horizontal, 30)
        .padding(.top, 14)
        .padding(.bottom, 16)
    }

    // MARK: step bodies
    @ViewBuilder private var bodyContent: some View {
        switch step {
        case 0:
            VStack(spacing: 0) {
                OrbCanvas(phase: .idle, cyFrac: 0.5)
                    .frame(height: 220)
                wizH(String(localized: "onboarding_welcome_title"))
                wizSub(String(localized: "onboarding_welcome_subtitle"))
            }
        case 1:
            VStack(spacing: 0) {
                badge(.sparkle)
                wizH(String(localized: "onboarding_assistant_title"))
                wizSub(String(localized: "onboarding_assistant_subtitle"))
                wizField(text: $assistantName, placeholder: String(localized: "app_name"))
                HStack {
                    HStack(spacing: 8) {
                        Circle().fill(Color(okl: 0.72, 0.18, 150)).frame(width: 7, height: 7)
                            .shadow(color: Color(okl: 0.72, 0.18, 150), radius: 4)
                        Text(assistantName.isEmpty ? String(localized: "app_name") : assistantName)
                            .font(.system(size: 14, weight: .medium)).foregroundColor(Color(okl: 0.88, 0.04, 285))
                    }
                    .padding(.horizontal, 16).padding(.vertical, 8)
                    .background(Color(okl: 0.70, 0.18, 285, 0.14), in: Capsule())
                    .overlay(Capsule().stroke(Color(okl: 0.70, 0.18, 285, 0.3), lineWidth: 0.5))
                }
                .frame(maxWidth: .infinity)
                .padding(.top, 18)
            }
        case 2:
            modelStep
        case 3:
            webStep
        case 4:
            voiceCapabilityStep
        default:
            VStack(spacing: 0) {
                badge(.brain)
                wizH(String(localized: "onboarding_done_title"))
                VStack(alignment: .leading, spacing: 12) {
                    completionRow(String(localized: "onboarding_done_row_assistant"), assistantName)
                    completionRow(
                        String(localized: "composer_current_model"),
                        providerRepository.settingsSummary.defaultModelID
                            ?? String(localized: "settings_provider_unconfigured")
                    )
                    completionRow(String(localized: "settings_web_search"), webProviderSummary(.search))
                    completionRow(String(localized: "settings_web_fetch"), webProviderSummary(.fetch))
                    completionRow(String(localized: "onboarding_done_row_voice"), voiceCapability.mode.title)
                    completionRow(String(localized: "settings_language_title"), voiceCapability.selectedLanguageLabel)
                    completionRow(String(localized: "onboarding_done_row_voiceover"), voiceCapability.effectiveVoiceLabel)
                }
                .padding(16)
                .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 15))
                .overlay(RoundedRectangle(cornerRadius: 15).stroke(Color.white.opacity(0.12), lineWidth: 0.5))
            }
        }
    }

    private var modelStep: some View {
        VStack(spacing: 0) {
            badge(.brain)
                .accessibilityIdentifier("onboarding.model.step")
            wizH(String(localized: "composer_setup_model"))
            wizSub(String(localized: "provider_llm_blurb"))

            if !providerRepository.profiles.isEmpty {
                VStack(spacing: 10) {
                    ForEach(providerRepository.profiles) { state in
                        providerProfileRow(state)
                    }
                }
                .padding(.bottom, 18)
            }

            VStack(alignment: .leading, spacing: 10) {
                Text(String(localized: "settings_title_add_llm"))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                    .frame(maxWidth: .infinity, alignment: .leading)

                LazyVStack(spacing: 10) {
                    ForEach(providerRepository.catalogPresets) { preset in
                        providerPresetRow(preset)
                    }
                }
            }
        }
    }

    private func providerProfileRow(_ state: ProviderProfileState) -> some View {
        setupLink(
            icon: .sparkle,
            title: state.profile.name,
            detail: state.profile.modelID,
            value: state.maskedCredentialSummary,
            accessibilityIdentifier: "onboarding.model.provider.\(state.id)"
        ) {
            editingProviderDraft = providerRepository.makeDraft(for: state.id)
        }
    }

    private func providerPresetRow(_ preset: ProviderPreset) -> some View {
        setupLink(
            icon: .plus,
            title: preset.name,
            detail: preset.sub,
            value: credentialMethodLabel(for: preset),
            accessibilityIdentifier: "onboarding.model.preset.\(preset.id)"
        ) {
            editingProviderDraft = providerRepository.makeNewDraft(presetID: preset.id)
        }
    }

    private func credentialMethodLabel(for preset: ProviderPreset) -> String {
        if preset.id == "openai-chatgpt" {
            return "OAuth"
        }
        if providerRepository.oauthLoginAvailable(for: preset.id) {
            return "API Key / OAuth"
        }
        return "API Key"
    }

    private var webStep: some View {
        VStack(spacing: 0) {
            badge(.search)
                .accessibilityIdentifier("onboarding.web.step")
            wizH(String(localized: "settings_web_search"))
            wizSub(String(localized: "provider_search_blurb"))
            VStack(spacing: 10) {
                setupLink(
                    icon: .search,
                    title: String(localized: "settings_web_search"),
                    detail: String(localized: "provider_search_blurb"),
                    value: webProviderSummary(.search),
                    accessibilityIdentifier: "onboarding.web.search"
                ) {
                    onOpenSettings(.providerList(.init(.search)))
                }
                setupLink(
                    icon: .link,
                    title: String(localized: "settings_web_fetch"),
                    detail: String(localized: "provider_fetch_blurb"),
                    value: webProviderSummary(.fetch),
                    accessibilityIdentifier: "onboarding.web.fetch"
                ) {
                    onOpenSettings(.providerList(.init(.fetch)))
                }
            }
        }
    }

    private func setupLink(
        icon: LXIconName,
        title: String,
        detail: String,
        value: String,
        accessibilityIdentifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 13) {
                LXIcon(name: icon, size: 19, color: .white, stroke: 1.8)
                    .frame(width: 40, height: 40)
                    .background(Color(okl: 0.70, 0.18, 285, 0.18), in: RoundedRectangle(cornerRadius: 11))
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(Color(okl: 0.95, 0.02, 285))
                    Text(detail)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Color(okl: 0.66, 0.03, 280))
                        .lineLimit(2)
                }
                Spacer(minLength: 8)
                VStack(alignment: .trailing, spacing: 5) {
                    Text(value)
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(Color(okl: 0.78, 0.05, 280))
                        .lineLimit(1)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.35))
                }
            }
            .padding(14)
            .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 15))
            .overlay {
                RoundedRectangle(cornerRadius: 15)
                    .stroke(.white.opacity(0.12), lineWidth: 0.5)
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(accessibilityIdentifier)
    }

    private func webProviderSummary(_ kind: ProviderKind) -> String {
        let providers = store.providers(kind)
        let provider = providers.first(where: { $0.isDefault && $0.enabled })
            ?? providers.first(where: \.enabled)
        return provider?.name ?? String(localized: "settings_provider_unconfigured")
    }

    private var voiceCapabilityStep: some View {
        VStack(spacing: 0) {
            badge(.mic)
            wizH(String(localized: "onboarding_voice_title"))
            wizSub(String(localized: "onboarding_voice_subtitle"))

            VStack(alignment: .leading, spacing: 10) {
                Text("settings_language_title")
                    .font(.caption.bold())
                    .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 8)], spacing: 8) {
                    voiceLanguageButton(
                        String(localized: "onboarding_voice_language_system"),
                        value: VoiceCapabilityModel.automaticLanguageIdentifier
                    )
                    voiceLanguageButton(String(localized: "onboarding_voice_language_zh"), value: "zh-CN")
                    voiceLanguageButton(String(localized: "onboarding_voice_language_en"), value: "en-US")
                    voiceLanguageButton(String(localized: "onboarding_voice_language_ja"), value: "ja-JP")
                }
                Text("voice_recognition_mode")
                    .font(.caption.bold())
                    .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                    .padding(.top, 8)
                ForEach(voiceCapability.recognitionModes) { mode in
                    voiceModeButton(mode)
                }

                if voiceCapability.mode == .onDevice,
                   !voiceCapability.offlinePackStates.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        Text("onboarding_voice_pack_title")
                            .font(.caption.bold())
                        Text("onboarding_voice_pack_subtitle")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        ForEach(voiceCapability.offlinePackStates) { status in
                            HStack {
                                Text(status.model.displayName["en"] ?? status.model.id)
                                Spacer()
                                Text(onboardingOfflineStateLabel(status.state))
                                    .foregroundStyle(status.state.isReady ? .green : .secondary)
                            }
                            .font(.caption)
                        }
                        Button(onboardingOfflineDownloadActive
                            ? String(localized: "common_cancel")
                            : String(localized: "voice_offline_download")) {
                            if onboardingOfflineDownloadActive {
                                voiceCapability.cancelOfflinePackDownload()
                            } else {
                                voiceCapability.downloadOfflinePack()
                            }
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    .padding(14)
                    .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 14))
                }

                Text("onboarding_voice_voice_title")
                    .font(.caption.bold())
                    .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                    .padding(.top, 8)

                HStack(spacing: 12) {
                    Picker("onboarding_voice_voice_title", selection: onboardingVoiceBinding) {
                        if voiceCapability.voices.isEmpty {
                            Text("onboarding_voice_no_voices").tag("")
                        } else {
                            ForEach(voiceCapability.voices) { voice in
                                Text("\(voice.name) · \(voice.language)").tag(voice.id)
                            }
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: .infinity, alignment: .leading)

                    Button(voiceCapability.isPreviewing ? "voice_playing" : "voice_preview") {
                        Task { await voiceCapability.preview() }
                    }
                    .buttonStyle(.bordered)
                    .disabled(voiceCapability.isPreviewing || voiceCapability.selectedVoice == nil)
                    .accessibilityIdentifier("onboarding.voice.preview")
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 14))
            }

            Text(voiceCapability.effectiveRecognitionLabel)
                .font(.footnote)
                .foregroundStyle(
                    voiceCapability.effectiveRecognitionStatus.fallbackReason != nil
                        ? Color.orange : Color(okl: 0.74, 0.10, 155)
                )
                .padding(.top, 18)

            if let error = voiceCapability.errorMessage {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.orange)
                    .multilineTextAlignment(.center)
                    .padding(.top, 10)
            }
        }
    }

    private var onboardingVoiceBinding: Binding<String> {
        Binding(
            get: { voiceCapability.selectedVoice?.id ?? "" },
            set: voiceCapability.setVoice
        )
    }

    private var onboardingOfflineDownloadActive: Bool {
        voiceCapability.offlinePackStates.contains { status in
            switch status.state {
            case .queued, .downloading, .verifying, .extracting: true
            default: false
            }
        }
    }

    private func onboardingOfflineStateLabel(_ state: VoiceModelState) -> String {
        switch state {
        case .notInstalled: String(localized: "voice_model_state_not_installed")
        case .queued: String(localized: "voice_model_state_queued")
        case .downloading: String(localized: "voice_model_state_downloading")
        case .verifying: String(localized: "voice_model_state_verifying")
        case .extracting: String(localized: "voice_model_state_installing")
        case .ready: String(localized: "voice_model_state_ready")
        case .failed: String(localized: "voice_model_state_failed")
        }
    }

    private func voiceLanguageButton(_ label: String, value: String) -> some View {
        Button(label) {
            voiceCapability.setLanguage(value)
        }
        .buttonStyle(.bordered)
        .tint(voiceCapability.language == value ? Color(okl: 0.70, 0.18, 285) : .gray)
    }

    private func voiceModeButton(_ mode: VoiceRecognitionMode) -> some View {
        Button {
            voiceCapability.setMode(mode)
        } label: {
            HStack(spacing: 12) {
                Image(systemName: voiceCapability.mode == mode ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(voiceCapability.mode == mode ? Color(okl: 0.70, 0.18, 285) : .gray)
                VStack(alignment: .leading, spacing: 3) {
                    Text(mode.title).font(.body.bold())
                    Text(mode.detail).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(14)
            .background(
                .white.opacity(voiceCapability.mode == mode ? 0.10 : 0.04),
                in: RoundedRectangle(cornerRadius: 14)
            )
        }
        .buttonStyle(.plain)
    }

    private func completionRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).foregroundStyle(Color(okl: 0.68, 0.04, 280))
            Spacer()
            Text(value).foregroundStyle(Color(okl: 0.95, 0.02, 285)).bold()
        }
        .font(.subheadline)
    }

    // MARK: small building blocks
    private func badge(_ icon: LXIconName) -> some View {
        LXIcon(name: icon, size: 30, color: .white, stroke: 1.9)
            .frame(width: 66, height: 66)
            .background(LinearGradient(colors: [Color(okl: 0.66, 0.20, 270), Color(okl: 0.62, 0.21, 312)], startPoint: .topLeading, endPoint: .bottomTrailing))
            .clipShape(RoundedRectangle(cornerRadius: 20))
            .shadow(color: Color(okl: 0.60, 0.20, 290, 0.4), radius: 16, y: 12)
            .padding(.bottom, 22)
    }
    private func wizH(_ text: String) -> some View {
        Text(text).font(.system(size: 25, weight: .bold)).kerning(0.4)
            .foregroundColor(Color(okl: 0.97, 0.02, 285))
            .multilineTextAlignment(.center).padding(.bottom, 10)
    }
    private func wizSub(_ text: String) -> some View {
        Text(text).font(.system(size: 14.5)).lineSpacing(4)
            .foregroundColor(Color(okl: 0.70, 0.03, 275))
            .multilineTextAlignment(.center).frame(maxWidth: 290).padding(.bottom, 26)
    }
    private func wizField(text: Binding<String>, placeholder: String) -> some View {
        TextField("", text: text, prompt: Text(placeholder).foregroundColor(Color(okl: 0.50, 0.02, 285)))
            .focused($fieldFocused)
            .multilineTextAlignment(.center)
            .font(.system(size: 21, weight: .semibold)).kerning(1)
            .foregroundColor(Color(okl: 0.96, 0.02, 285))
            .padding(.horizontal, 18).padding(.vertical, 16)
            .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 15))
            .overlay(RoundedRectangle(cornerRadius: 15).stroke(.white.opacity(0.18), lineWidth: 0.5))
            .submitLabel(.next)
    }

    // MARK: CTA state
    private var cta: String {
        switch step {
        case 0: return String(localized: "onboarding_cta_start")
        case 4: return String(localized: "onboarding_cta_continue")
        case Self.total - 1: return String(localized: "onboarding_cta_finish")
        default: return String(localized: "onboarding_cta_continue")
        }
    }
    private var ctaDisabled: Bool {
        switch step {
        case 1: return assistantName.trimmingCharacters(in: .whitespaces).isEmpty
        default: return false
        }
    }
    private var skipLabel: String? { nil }

    // MARK: actions
    private func next() {
        if step < Self.total - 1 {
            fieldFocused = false
            withAnimation(.easeOut(duration: 0.3)) { step += 1 }
        } else {
            finish()
        }
    }
    private func finish() {
        app.assistantName = assistantName.trimmingCharacters(in: .whitespaces)
        app.setupDone = true
        onDone()
    }
}
