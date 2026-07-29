import SwiftUI

// MARK: - First-run setup wizard
//
// Port of the prototype's `SetupWizard` (lingxi-iphone.html). A 5-step first-run
// flow over the sci-fi orb backdrop: welcome → name the assistant (wake word) →
// your name → enroll a voiceprint (optional, simulated) → pick a default model.
// On finish it writes the chosen values to `AppState` and marks `setupDone`, so
// it only runs once (re-triggerable from Settings → 关于 → 重新观看引导).

struct SetupWizardView: View {
    @EnvironmentObject private var app: AppState
    /// The shared conversation model — its `availableModels` / `activeModelId`
    /// carry the engine's REAL model catalog (out-of-band). The model step renders
    /// only these references; empty means the first `ModelList` has not landed.
    @ObservedObject var convo: ConversationModel
    /// Commit the chosen model to the engine (`SetModel`) when it is a real id.
    var onSetModel: (String) -> Void = { _ in }
    /// Called once the wizard commits (or is finished) — RootView dismisses it
    /// by reading `app.setupDone`; this is the hook for any extra teardown.
    var onDone: () -> Void = {}

    private static let total = 5

    /// One model row, keeping the full provider-qualified selection reference.
    private struct WizModel: Identifiable { let id: String; let name: String; let sub: String; let color: Color }
    private struct WizModelSection: Identifiable {
        let id: String
        let name: String
        let models: [WizModel]
    }

    /// Group only the curated references received from the engine. The wizard
    /// never fills a section from the Provider's complete remote catalog.
    private var wizardModelSections: [WizModelSection] {
        ModelDisplay.sections(for: convo.availableModels).map { section in
            WizModelSection(
                id: section.providerId,
                name: section.name,
                models: section.models.map { item in
                    WizModel(
                        id: item.reference,
                        name: item.name,
                        sub: item.modelId,
                        color: item.color)
                })
        }
    }

    private var wizardModels: [WizModel] {
        wizardModelSections.flatMap(\.models)
    }

    /// The effective selection: the user's pick when it's in the catalog, else the
    /// engine's active id, else the first row (keeps a valid default as the real
    /// catalog arrives async during first-run).
    private var selectedModelId: String {
        if wizardModels.contains(where: { $0.id == modelId }) { return modelId }
        if !convo.activeModelId.isEmpty { return convo.activeModelId }
        return wizardModels.first?.id ?? modelId
    }

    @State private var step = 0
    @State private var seeded = false

    // Editable copies, seeded from AppState on first appear.
    @State private var assistantName = "灵犀"
    @State private var userName = ""
    @State private var modelId = ""

    // Voiceprint enrollment simulation: idle | rec | done.
    private enum VP { case idle, rec, done }
    @State private var vp: VP = .idle
    @State private var vpPct: Double = 0
    @State private var vpTimer: Timer?

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
                ScrollView {
                    bodyContent
                        .padding(.horizontal, 30)
                        .id(step)
                        .transition(.asymmetric(insertion: .move(edge: .trailing).combined(with: .opacity),
                                                removal: .opacity))
                }
                .scrollDismissesKeyboard(.interactively)
                footer
            }
        }
        .onAppear {
            guard !seeded else { return }
            seeded = true
            assistantName = app.assistantName
            userName = app.userName
            modelId = app.defaultModelId
            vp = app.voiceprint ? .done : .idle
            vpPct = app.voiceprint ? 100 : 0
        }
        .onDisappear { vpTimer?.invalidate() }
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

            if let skip = skipLabel {
                Button(skip) { withAnimation(.easeOut(duration: 0.3)) { step += 1 } }
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundColor(Color(okl: 0.60, 0.03, 275))
            } else {
                Color.clear.frame(height: 20)
            }
        }
        .padding(.horizontal, 30)
        .padding(.top, 14)
        .padding(.bottom, 42)
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
            voiceprintStep
        default:
            VStack(spacing: 0) {
                badge(.brain)
                wizH("选择默认模型")
                wizSub("随时可在对话中切换。不确定就先用推荐的主力模型。")
                VStack(alignment: .leading, spacing: 14) {
                    if wizardModelSections.isEmpty {
                        Text("正在从引擎加载可用模型…")
                            .font(.system(size: 13))
                            .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                            .frame(maxWidth: .infinity, alignment: .center)
                            .padding(.vertical, 20)
                    }
                    ForEach(wizardModelSections) { section in
                        VStack(alignment: .leading, spacing: 8) {
                            Text(section.name)
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(Color(okl: 0.68, 0.04, 280))
                                .textCase(.uppercase)
                                .padding(.leading, 4)
                            ForEach(section.models) { model in
                                modelRow(model)
                            }
                        }
                    }
                }
            }
        }
    }

    private var voiceprintStep: some View {
        VStack(spacing: 0) {
            badge(.mic)
            wizH("录入你的声纹")
            wizSub("让灵犀听声识人，只对你的声音响应、唤起属于你的记忆。可稍后在设置里完成。")
            ZStack {
                Circle().stroke(.white.opacity(0.1), lineWidth: 4).frame(width: 108, height: 108)
                Circle().trim(from: 0, to: CGFloat(vpPct / 100))
                    .stroke(Color(okl: 0.70, 0.19, 290), style: StrokeStyle(lineWidth: 4, lineCap: .round))
                    .frame(width: 108, height: 108)
                    .rotationEffect(.degrees(-90))
                    .shadow(color: Color(okl: 0.70, 0.19, 290, 0.7), radius: 4)
                Button(action: startVoiceprint) {
                    LXIcon(name: vp == .done ? .check : .mic, size: 34, color: .white, stroke: 2)
                        .frame(width: 88, height: 88)
                        .background {
                            if vp == .done {
                                LinearGradient(colors: [Color(okl: 0.70, 0.16, 150), Color(okl: 0.64, 0.16, 165)], startPoint: .topLeading, endPoint: .bottomTrailing)
                            } else {
                                LinearGradient(colors: [Color(okl: 0.66, 0.20, 270), Color(okl: 0.62, 0.21, 312)], startPoint: .topLeading, endPoint: .bottomTrailing)
                            }
                        }
                        .clipShape(Circle())
                        .shadow(color: Color(okl: 0.60, 0.20, 290, 0.4), radius: 10, y: 4)
                }
                .disabled(vp == .rec)
            }
            .frame(width: 140, height: 140)
            .padding(.top, 6)
            Text(vpStatusText)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(vp == .done ? Color(okl: 0.74, 0.15, 155) : Color(okl: 0.78, 0.04, 280))
                .padding(.top, 20)
                .frame(minHeight: 22)
            if vp != .done {
                Text("「你好灵犀，我是\(userName.isEmpty ? "我" : userName)。」")
                    .font(.system(size: 15)).italic()
                    .foregroundColor(Color(okl: 0.86, 0.03, 280))
                    .padding(.top, 8)
            }
        }
    }

    private var vpStatusText: String {
        switch vp {
        case .idle: return "轻点麦克风，朗读下面这句话"
        case .rec:  return "正在聆听你的声音… \(Int(vpPct))%"
        case .done: return "✓ 声纹已录入"
        }
    }

    private func modelRow(_ m: WizModel) -> some View {
        let on = selectedModelId == m.id
        return Button { modelId = m.id } label: {
            HStack(spacing: 13) {
                Circle().fill(m.color).frame(width: 10, height: 10).shadow(color: m.color, radius: 4)
                VStack(alignment: .leading, spacing: 2) {
                    Text(m.name).font(.system(size: 15.5, weight: .semibold)).foregroundColor(Color(okl: 0.95, 0.02, 285))
                    Text(m.sub).font(.system(size: 12.5)).foregroundColor(Color(okl: 0.66, 0.03, 280)).lineLimit(1)
                }
                Spacer(minLength: 0)
                ZStack {
                    Circle().stroke(on ? Color(okl: 0.70, 0.18, 285) : .white.opacity(0.25), lineWidth: 1.5)
                        .frame(width: 22, height: 22)
                    if on { Circle().fill(Color(okl: 0.66, 0.20, 288)).frame(width: 22, height: 22)
                        LXIcon(name: .check, size: 13, color: .white, stroke: 3) }
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 14)
            .background(on ? Color(okl: 0.70, 0.18, 285, 0.16) : .white.opacity(0.04), in: RoundedRectangle(cornerRadius: 15))
            .overlay(RoundedRectangle(cornerRadius: 15).stroke(on ? Color(okl: 0.70, 0.18, 285, 0.55) : .white.opacity(0.1), lineWidth: 1))
        }
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
        case 3: return vp == .done ? "继续" : "稍后再说"
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
    private var skipLabel: String? {
        (step == 3 && vp != .done) ? "跳过此步" : nil
    }

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
        let chosen = selectedModelId
        app.assistantName = assistantName.trimmingCharacters(in: .whitespaces)
        app.userName = userName.trimmingCharacters(in: .whitespaces)
        app.voiceprint = (vp == .done)
        app.setupDone = true
        // When the engine's real catalog is present, commit the pick to it.
        if !convo.availableModels.isEmpty {
            app.defaultModelId = chosen
            onSetModel(chosen)
        }
        onDone()
    }

    private func startVoiceprint() {
        guard vp != .rec else { return }
        vp = .rec; vpPct = 0
        let start = Date()
        vpTimer?.invalidate()
        vpTimer = Timer.scheduledTimer(withTimeInterval: 0.04, repeats: true) { tmr in
            let p = min(100, Date().timeIntervalSince(start) / 2.6 * 100)
            vpPct = p
            if p >= 100 { tmr.invalidate(); vp = .done }
        }
    }
}
