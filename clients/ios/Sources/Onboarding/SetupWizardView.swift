import SwiftUI

// MARK: - First-run setup wizard
//
// Port of the prototype's `SetupWizard` (lingxi-iphone.html). A 5-step first-run
// flow over the sci-fi orb backdrop: welcome → name the assistant (wake word) →
// your name → choose native speech behavior → finish. Provider/model setup stays
// in the real Provider settings flow, so onboarding never presents mock models.
// On finish it writes the chosen values to `AppState` and marks `setupDone`, so
// it only runs once (re-triggerable from Settings → 关于 → 重新观看引导).

struct SetupWizardView: View {
    @Environment(AppState.self) private var app
    /// Kept in the initializer for source compatibility with the existing root;
    /// model selection now belongs to the real Provider settings flow.
    @ObservedObject var convo: ConversationModel
    /// Commit the chosen model to the engine (`SetModel`) when it is a real id.
    var onSetModel: (String) -> Void = { _ in }
    /// Called once the wizard commits (or is finished) — RootView dismisses it
    /// by reading `app.setupDone`; this is the hook for any extra teardown.
    var onDone: () -> Void = {}

    private static let total = 5

    @State private var step = 0
    @State private var seeded = false

    // Editable copies, seeded from AppState on first appear.
    @State private var assistantName = "灵犀"
    @State private var userName = ""
    @State private var recognitionMode: VoiceRecognitionMode = .onDevice
    @State private var voiceLanguage = VoiceCapabilityModel.automaticLanguageIdentifier
    @State private var voiceCapability = VoiceCapabilityModel()

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
            userName = app.userName
            recognitionMode = VoiceRecognitionMode(rawValue: app.voiceRecognitionMode) ?? .onDevice
            voiceLanguage = app.voiceLanguage
            voiceCapability.setMode(recognitionMode)
            voiceCapability.setLanguage(voiceLanguage)
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
                .accessibilityLabel("返回")
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
            Color.clear.frame(width: 36)
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
                wizH("欢迎使用灵犀")
                wizSub("花一分钟，让它认识你——之后你只需开口，它就会回应。")
            }
        case 1:
            VStack(spacing: 0) {
                badge(.sparkle)
                wizH("给你的灵犀起个名字")
                wizSub("这会成为它的唤醒词。之后你可以说「嘿，\(assistantName.isEmpty ? "灵犀" : assistantName)」随时唤醒它。")
                wizField(text: $assistantName, placeholder: "灵犀")
                HStack {
                    HStack(spacing: 8) {
                        Circle().fill(Color(okl: 0.72, 0.18, 150)).frame(width: 7, height: 7)
                            .shadow(color: Color(okl: 0.72, 0.18, 150), radius: 4)
                        Text("嘿，\(assistantName.isEmpty ? "灵犀" : assistantName)")
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
            VStack(spacing: 0) {
                badge(.skill)
                wizH("我该怎么称呼你？")
                wizSub("灵犀会用这个名字称呼你，让对话更自然亲切。")
                wizField(text: $userName, placeholder: "你的名字")
            }
        case 3:
            voiceCapabilityStep
        default:
            VStack(spacing: 0) {
                badge(.brain)
                wizH("基础设置完成")
                wizSub("进入应用后，请在「设置 → LLM 提供商」保存真实凭据并选择默认模型。")
                VStack(alignment: .leading, spacing: 12) {
                    completionRow("助手", assistantName)
                    completionRow("称呼", userName)
                    completionRow("语音", recognitionMode.title)
                    completionRow("语言", VoiceCapabilityModel.displayName(for: voiceLanguage))
                }
                .padding(16)
                .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 15))
                .overlay(RoundedRectangle(cornerRadius: 15).stroke(Color.white.opacity(0.12), lineWidth: 0.5))
            }
        }
    }

    private var voiceCapabilityStep: some View {
        VStack(spacing: 0) {
            badge(.mic)
            wizH("选择语音识别方式")
            wizSub("优先使用 iOS 设备端识别；当前语言不支持时会明确回退到系统识别。")

            VStack(alignment: .leading, spacing: 10) {
                Text("语言")
                    .font(.caption.bold())
                    .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 8)], spacing: 8) {
                    voiceLanguageButton(
                        "跟随系统",
                        value: VoiceCapabilityModel.automaticLanguageIdentifier
                    )
                    voiceLanguageButton("中文", value: "zh-CN")
                    voiceLanguageButton("English", value: "en-US")
                    voiceLanguageButton("日本語", value: "ja-JP")
                }
                Text("识别方式")
                    .font(.caption.bold())
                    .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                    .padding(.top, 8)
                ForEach(VoiceRecognitionMode.allCases) { mode in
                    voiceModeButton(mode)
                }
            }

            Text(voiceCapability.effectiveRecognitionLabel)
                .font(.footnote)
                .foregroundStyle(
                    recognitionMode == .onDevice && !voiceCapability.onDeviceAvailable
                        ? Color.orange : Color(okl: 0.74, 0.10, 155)
                )
                .padding(.top, 18)
        }
    }

    private func voiceLanguageButton(_ label: String, value: String) -> some View {
        Button(label) {
            voiceLanguage = value
            voiceCapability.setLanguage(value)
        }
        .buttonStyle(.bordered)
        .tint(voiceLanguage == value ? Color(okl: 0.70, 0.18, 285) : .gray)
    }

    private func voiceModeButton(_ mode: VoiceRecognitionMode) -> some View {
        Button {
            recognitionMode = mode
            voiceCapability.setMode(mode)
        } label: {
            HStack(spacing: 12) {
                Image(systemName: recognitionMode == mode ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(recognitionMode == mode ? Color(okl: 0.70, 0.18, 285) : .gray)
                VStack(alignment: .leading, spacing: 3) {
                    Text(mode.title).font(.body.bold())
                    Text(mode.detail).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(14)
            .background(
                .white.opacity(recognitionMode == mode ? 0.10 : 0.04),
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
        case 0: return "开始设置"
        case 3: return "继续"
        case Self.total - 1: return "进入灵犀"
        default: return "继续"
        }
    }
    private var ctaDisabled: Bool {
        switch step {
        case 1: return assistantName.trimmingCharacters(in: .whitespaces).isEmpty
        case 2: return userName.trimmingCharacters(in: .whitespaces).isEmpty
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
        app.userName = userName.trimmingCharacters(in: .whitespaces)
        app.voiceRecognitionMode = recognitionMode.rawValue
        app.voiceLanguage = voiceLanguage
        app.setupDone = true
        onDone()
    }
}
