import AVFoundation
import SwiftUI

// MARK: - FlowMode voice orb ("心流" — living LLM presence)
//
// Port of the prototype's `OrbCanvas` + `VoiceOrb` (lingxi-iphone.html). The orb
// is a Canvas render loop driven by `TimelineView(.animation)`: a deep-space
// starfield, a field of orbiting particles, a fluid blob body with a radial core
// + inner glow blobs + specular highlight, three wobbling membrane rings, and a
// thinking arc. Each phase eases toward a SUSTAINED size (no constant pulsing) —
// energy reads as blob wobble + particle motion rather than size jitter.
//
// `VoiceOrb` wraps it with the scripted conversational flow (idle → listening →
// thinking → speaking), char-by-char captions, a status label, and the optional
// pop-up text input.

enum OrbPhase { case idle, listening, thinking, speaking }
enum OrbRole { case user, ai }

// MARK: - Orb simulation state (persists across frames, mutated in the draw loop)

private struct OrbParticle { var a: Double; var rad: Double; var sp: Double; var sz: Double; var tw: Double }
private struct OrbStar { var x: Double; var y: Double; var z: Double; var tw: Double }

/// A tiny deterministic RNG so the particle/star field is stable across launches
/// (the prototype seeds with Math.random; we just want a fixed, pleasing spread).
private struct LCG { var s: UInt64; mutating func next() -> Double { s = s &* 6364136223846793005 &+ 1442695040888963407; return Double(s >> 11) / Double(1 << 53) } }

private final class OrbSim {
    var started = false
    var lastTime: Double = 0
    var tm: Double = 0
    var amp: Double = 0.12
    var hueShift: Double = 0
    var baseScale: Double = 1
    var breath: Double = 0
    var parts: [OrbParticle] = []
    var stars: [OrbStar] = []

    init() {
        var r = LCG(s: 0x9E3779B97F4A7C15)
        let tau = Double.pi * 2
        parts = (0..<76).map { _ in
            OrbParticle(a: r.next() * tau,
                        rad: 0.55 + r.next() * 1.5,
                        sp: (0.1 + r.next() * 0.5) * (r.next() < 0.5 ? 1 : -1),
                        sz: 0.6 + r.next() * 1.8,
                        tw: r.next() * tau)
        }
        stars = (0..<90).map { _ in
            OrbStar(x: r.next(), y: r.next(), z: 0.25 + r.next() * 0.75, tw: r.next() * tau)
        }
    }
}

// MARK: - OrbCanvas

struct OrbCanvas: View {
    var phase: OrbPhase
    var cyFrac: CGFloat = 0.40

    @State private var sim = OrbSim()

    var body: some View {
        TimelineView(.animation) { timeline in
            Canvas { ctx, size in
                draw(&ctx, size, now: timeline.date.timeIntervalSinceReferenceDate)
            }
        }
    }

    // Closed fluid-blob outline (cx,cy ± harmonic wobble).
    private func blob(_ cx: Double, _ cy: Double, _ base: Double, _ wob: Double, _ seed: Double, _ harm: Double) -> Path {
        var p = Path()
        let tau = Double.pi * 2
        let steps = 132
        let tm = sim.tm
        for i in 0...steps {
            let a = Double(i) / Double(steps) * tau
            let r = base
                + sin(a * 3 + tm * 1.3 + seed) * wob * 0.5
                + sin(a * 5 - tm * 1.9 + seed * 1.7) * wob * 0.32
                + sin(a * 2 + tm * 0.9 + seed * 0.4) * wob * 0.42
                + sin(a * 7 + tm * 2.4) * wob * 0.16 * harm
            let x = cx + cos(a) * r, y = cy + sin(a) * r
            if i == 0 { p.move(to: CGPoint(x: x, y: y)) } else { p.addLine(to: CGPoint(x: x, y: y)) }
        }
        p.closeSubpath()
        return p
    }

    private func draw(_ ctx: inout GraphicsContext, _ size: CGSize, now: Double) {
        if !sim.started { sim.started = true; sim.lastTime = now }
        let dt = min(0.05, now - sim.lastTime); sim.lastTime = now; sim.tm += dt
        let W = Double(size.width), H = Double(size.height)
        let cx = W / 2, cy = H * Double(cyFrac)
        let baseR = min(W, H) * 0.165

        // Per-phase settled targets (size eases toward target; energy → wobble).
        let baseTarget: Double, breathDepth: Double, breathSpeed: Double, wobTarget: Double
        switch phase {
        case .idle:      baseTarget = 1.0;  breathDepth = 0.03;  breathSpeed = 0.9; wobTarget = 0.10
        case .listening: baseTarget = 1.07; breathDepth = 0.04;  breathSpeed = 1.5; wobTarget = 0.16
        case .thinking:  baseTarget = 1.04; breathDepth = 0.03;  breathSpeed = 1.2; wobTarget = 0.13; sim.hueShift += dt * 40
        case .speaking:  baseTarget = 1.26; breathDepth = 0.035; breathSpeed = 1.8; wobTarget = 0.22
        }
        sim.baseScale += (baseTarget - sim.baseScale) * min(1, dt * 4.0)
        sim.breath += dt * breathSpeed
        let scale = sim.baseScale + sin(sim.breath) * breathDepth
        let R = baseR * scale
        sim.amp += (wobTarget - sim.amp) * min(1, dt * 3)
        let A = max(0, sim.amp)

        // ── deep-space starfield ──
        for s in sim.stars {
            let sx = ((s.x + sim.tm * 0.004 * s.z).truncatingRemainder(dividingBy: 1)) * W
            let sy = s.y * H
            let twk = 0.35 + 0.65 * (0.5 + 0.5 * sin(sim.tm * (1 + s.z * 2) + s.tw))
            let ss = s.z > 0.8 ? 1.6 : 1.0
            let c = Color(okl: 0.78 + s.z * 0.18, 0.03, 250 + s.z * 60, twk * (0.12 + s.z * 0.38))
            ctx.fill(Path(CGRect(x: sx, y: sy, width: ss, height: ss)), with: .color(c))
        }

        // ── orbiting particle field (additive) ──
        ctx.blendMode = .plusLighter
        let conv = phase == .thinking ? 0.6 : 1.0
        for i in sim.parts.indices {
            sim.parts[i].a += sim.parts[i].sp * dt * (phase == .idle ? 0.4 : 1)
            let pt = sim.parts[i]
            let rr = R * pt.rad * conv * (1 + A * 0.25)
            let x = cx + cos(pt.a) * rr * 1.15
            let y = cy + sin(pt.a) * rr
            let tw = 0.4 + 0.6 * (sin(sim.tm * 2 + pt.tw) * 0.5 + 0.5)
            let hue = 250 + 80 * (pt.rad - 0.55) / 1.5 + sim.hueShift
            let c = Color(okl: 0.85, 0.16, hue, tw * (0.5 + A * 0.4))
            let psz = pt.sz * (0.7 + A * 0.5)
            ctx.fill(Path(ellipseIn: CGRect(x: x - psz, y: y - psz, width: psz * 2, height: psz * 2)), with: .color(c))
        }

        let wob = R * (0.05 + A * 0.24)

        // ── blob body: clipped core gradient + inner glow blobs + specular ──
        ctx.blendMode = .normal
        let clipPath = blob(cx, cy, R * 0.94, wob, 0, 1)
        ctx.drawLayer { layer in paintCore(&layer, clip: clipPath, cx: cx, cy: cy, R: R, A: A) }

        paintRings(&ctx, cx: cx, cy: cy, R: R, A: A, wob: wob)
        paintThinkingArc(&ctx, cx: cx, cy: cy, R: R)
        ctx.blendMode = .normal
    }

    /// Clipped blob body: radial core gradient + inner glow blobs + specular.
    private func paintCore(_ layer: inout GraphicsContext, clip: Path, cx: Double, cy: Double, R: Double, A: Double) {
        layer.clip(to: clip)
        let coreH: Double = 282 + 30 * sin(sim.tm * 0.5) + (phase == .thinking ? sim.hueShift * 0.3 : 0)
        let cs0 = Color(okl: 0.97, 0.04, coreH, 0.95)
        let cs1 = Color(okl: 0.80, 0.18, coreH, 0.92)
        let cs2 = Color(okl: 0.58, 0.21, coreH + 20, 0.7)
        let cs3 = Color(okl: 0.40, 0.18, coreH + 30, 0.15)
        let coreGrad = Gradient(stops: [
            .init(color: cs0, location: 0),
            .init(color: cs1, location: 0.32),
            .init(color: cs2, location: 0.7),
            .init(color: cs3, location: 1),
        ])
        let coreRect = CGRect(x: cx - R * 2, y: cy - R * 2, width: R * 4, height: R * 4)
        layer.fill(Path(coreRect), with: .radialGradient(coreGrad, center: CGPoint(x: cx, y: cy - R * 0.28), startRadius: R * 0.05, endRadius: R * 1.15))
        layer.blendMode = .plusLighter
        let glowHues = [260.0, 320.0, 195.0]
        for k in 0..<3 {
            let ya = cy + sin(sim.tm * (1.1 + Double(k) * 0.5) + Double(k)) * R * 0.4 * (0.4 + A)
            let g0 = Color(okl: 0.92, 0.14, glowHues[k] + sim.hueShift, 0.18 + A * 0.22)
            let g1 = Color(okl: 0.90, 0.10, 270, 0)
            let g = Gradient(stops: [.init(color: g0, location: 0), .init(color: g1, location: 1)])
            let rect = CGRect(x: cx - R * 0.9, y: ya - R * 0.9, width: R * 1.8, height: R * 1.8)
            layer.fill(Path(ellipseIn: rect), with: .radialGradient(g, center: CGPoint(x: cx, y: ya), startRadius: 0, endRadius: R * 0.9))
        }
        let spec = Color(okl: 0.99, 0.02, 270, 0.5 + A * 0.3)
        let sr = R * 0.14
        let specRect = CGRect(x: cx - R * 0.28 - sr, y: cy - R * 0.34 - sr, width: sr * 2, height: sr * 2)
        layer.fill(Path(ellipseIn: specRect), with: .color(spec))
    }

    /// Three wobbling membrane rings around the body.
    private func paintRings(_ ctx: inout GraphicsContext, cx: Double, cy: Double, R: Double, A: Double, wob: Double) {
        ctx.blendMode = .plusLighter
        let ringH = [268.0, 322.0, 196.0]
        for k in 0..<3 {
            let path = blob(cx, cy, R * (1.0 + Double(k) * 0.07), wob * (1 + Double(k) * 0.35), Double(k) * 2.3, 1)
            let c = Color(okl: 0.85, 0.17, ringH[k] + sim.hueShift * 0.3, 0.55 - Double(k) * 0.14 + A * 0.2)
            ctx.stroke(path, with: .color(c), lineWidth: 1.6 - Double(k) * 0.45)
        }
    }

    /// A single rotating arc shown while thinking.
    private func paintThinkingArc(_ ctx: inout GraphicsContext, cx: Double, cy: Double, R: Double) {
        guard phase == .thinking else { return }
        let st = sim.tm * 3.2
        var arc = Path()
        arc.addArc(center: CGPoint(x: cx, y: cy), radius: R * 1.32,
                   startAngle: .radians(st), endAngle: .radians(st + .pi * 1.1), clockwise: false)
        ctx.stroke(arc, with: .color(Color(okl: 0.88, 0.16, 260 + sim.hueShift, 0.85)),
                   style: StrokeStyle(lineWidth: 2, lineCap: .round))
    }
}

// MARK: - VoiceOrb overlay

struct VoiceOrbView: View {
    @EnvironmentObject private var app: AppState
    /// The REAL conversation model — its `messages` / `streaming` drive the orb's
    /// thinking→speaking captions exactly as they drive ChatView (the orb is just
    /// another view of the same live session).
    @ObservedObject var convo: ConversationModel
    /// Submit a user turn to the engine (`source.send`).
    var onSend: (String) -> Void = { _ in }
    /// Interrupt the in-flight turn (`source.cancel`).
    var onCancel: () -> Void = {}
    let onClose: () -> Void

    @State private var phase: OrbPhase = .idle
    @State private var userCaption = ""
    @State private var didSend = false
    @State private var listenTask: Task<Void, Never>? = nil
    @State private var spokenForText = ""
    @State private var speaker = AVSpeechSynthesizer()
    private let voiceCapture = VoiceCapture()

    @State private var typing = false
    @State private var draft = ""
    @FocusState private var inputFocused: Bool

    private var assistantName: String {
        app.assistantName.isEmpty ? "灵犀" : app.assistantName
    }
    /// The assistant reply for the in-flight/just-finished turn — the last message
    /// when it is the AI's (grows live as `TextDelta`s land into `convo.messages`).
    private var assistantText: String {
        if let last = convo.messages.last, last.role == .ai { return last.text }
        return ""
    }
    private var captionIsAI: Bool { phase == .speaking || (phase == .idle && didSend) }
    private var displayCaption: String {
        switch phase {
        case .listening: return ""
        case .thinking:  return userCaption
        case .speaking:  return assistantText
        case .idle:      return didSend ? assistantText : ""
        }
    }
    private var label: String {
        switch phase {
        case .idle, .listening: return "聆听中"
        case .thinking: return "思考中"
        case .speaking: return assistantName
        }
    }
    private var sub: String {
        switch phase {
        case .idle: return ""
        case .listening: return "说完轻点收音 · 或继续"
        case .thinking: return "正在组织语言…"
        case .speaking: return "轻点光球可打断"
        }
    }

    var body: some View {
        content
            .transition(.opacity)
            .onAppear { listen() }
            .onDisappear { teardown() }
            // First assistant delta of the turn flips thinking → speaking.
            .onChange(of: assistantText) { _, text in onAssistantText(text) }
            // Turn end (streaming → false) settles the orb and speaks the reply.
            .onChange(of: convo.streaming) { _, streaming in onStreamingChanged(streaming) }
    }

    private var content: some View {
        ZStack {
            backdrop
            orbLayer
            VStack(spacing: 0) {
                topBar
                Spacer()
                statusAndCaption
            }
            if typing { textInputOverlay }
        }
    }

    private func teardown() {
        listenTask?.cancel()
        onCancel()
        speaker.stopSpeaking(at: .immediate)
    }
    private func onAssistantText(_ text: String) {
        if phase == .thinking && !text.isEmpty { phase = .speaking }
    }
    private func onStreamingChanged(_ streaming: Bool) {
        if streaming {
            if phase == .thinking && !assistantText.isEmpty { phase = .speaking }
        } else if phase == .thinking || phase == .speaking {
            phase = .idle
            speak(assistantText)
        }
    }

    // Fixed sci-fi backdrop (independent of light/dark theme).
    private var backdrop: some View {
        ZStack {
            RadialGradient(colors: [Color(okl: 0.20, 0.07, 270), Color(srgb: 0.020, 0.020, 0.039)],
                           center: UnitPoint(x: 0.5, y: 0.32), startRadius: 0, endRadius: 520)
                .ignoresSafeArea()
            RadialGradient(colors: [.clear, .black.opacity(0.55)],
                           center: UnitPoint(x: 0.5, y: 0.40), startRadius: 120, endRadius: 460)
                .ignoresSafeArea()
                .allowsHitTesting(false)
        }
    }

    // The orb — tap to interrupt / re-listen.
    private var orbLayer: some View {
        OrbCanvas(phase: phase, cyFrac: 0.40)
            .ignoresSafeArea()
            .contentShape(Rectangle())
            .onTapGesture { listen() }
    }

    // MARK: real-voice driver
    /// Start a one-shot listen → send → (engine streams the reply) cycle.
    private func listen() {
        onCancel()
        speaker.stopSpeaking(at: .immediate)
        listenTask?.cancel()
        userCaption = ""; didSend = false; phase = .listening
        listenTask = Task {
            let result = await voiceCapture.transcribe()
            await MainActor.run {
                switch result {
                case .transcript(let text): submitTurn(text)
                default: if phase == .listening { phase = .idle }
                }
            }
        }
    }
    /// Send recognized/typed text as a real engine turn.
    private func submitTurn(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { phase = .idle; return }
        userCaption = t; didSend = true; phase = .thinking
        onSend(t)
    }
    /// Speak the assistant reply aloud (best-effort device TTS, once per reply).
    private func speak(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty, t != spokenForText else { return }
        spokenForText = t
        let u = AVSpeechUtterance(string: t)
        u.voice = AVSpeechSynthesisVoice(language: "zh-CN")
        speaker.speak(u)
    }

    private var topBar: some View {
        HStack {
            Button(action: onClose) {
                LXIcon(name: .chevron, size: 20, color: Color(okl: 0.90, 0.01, 270), stroke: 2)
                    .frame(width: 40, height: 40)
                    .background(.white.opacity(0.06), in: Circle())
                    .overlay(Circle().stroke(.white.opacity(0.12), lineWidth: 0.5))
            }
            .accessibilityLabel("退出心流")
            Spacer()
            if app.inputDialog {
                Button { typing = true; inputFocused = true } label: {
                    LXIcon(name: .message, size: 18, color: Color(okl: 0.90, 0.01, 270), stroke: 1.8)
                        .frame(width: 40, height: 40)
                        .background(.white.opacity(0.06), in: Circle())
                        .overlay(Circle().stroke(.white.opacity(0.12), lineWidth: 0.5))
                }
                .accessibilityLabel("文字输入")
            } else {
                Color.clear.frame(width: 40, height: 40)
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 8)
    }

    private var statusAndCaption: some View {
        VStack(spacing: 14) {
            HStack(spacing: 8) {
                Circle()
                    .fill(dotColor)
                    .frame(width: 7, height: 7)
                    .shadow(color: dotColor, radius: 5)
                    .opacity(phase == .idle ? 1 : 0.85)
                Text(label)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color(okl: 0.92, 0.02, 270))
                    .kerning(0.5)
            }
            captionBlock
                .frame(minHeight: 84)
                .frame(maxWidth: 320)
                .id("\(label)\(captionIsAI ? "ai" : "user")")
                .transition(.opacity)
        }
        .padding(.horizontal, 28)
        .padding(.bottom, 46)
        .allowsHitTesting(false)
    }

    @ViewBuilder private var captionBlock: some View {
        if displayCaption.isEmpty {
            Text(sub)
                .font(.system(size: 14))
                .foregroundColor(Color(okl: 0.60, 0.03, 270))
                .multilineTextAlignment(.center)
        } else {
            let isAI = captionIsAI
            (Text(isAI ? "" : "“").foregroundColor(Color(okl: 0.62, 0.04, 270))
                + Text(displayCaption)
                + Text(isAI ? "" : "”").foregroundColor(Color(okl: 0.62, 0.04, 270)))
                .font(.system(size: isAI ? 18 : 17, weight: isAI ? .medium : .regular))
                .foregroundColor(isAI ? Color(okl: 0.96, 0.02, 290) : Color(okl: 0.80, 0.03, 270))
                .lineSpacing(5)
                .multilineTextAlignment(.center)
        }
    }

    private var dotColor: Color {
        switch phase {
        case .listening: return Color(okl: 0.72, 0.18, 150)
        case .speaking:  return Color(okl: 0.75, 0.19, 300)
        default:         return Color(okl: 0.70, 0.16, 260)
        }
    }

    // Pop-up text input (心流中的文字输入).
    private var textInputOverlay: some View {
        ZStack(alignment: .bottom) {
            Color.black.opacity(0.55).ignoresSafeArea()
                .onTapGesture { typing = false }
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    LXIcon(name: .message, size: 14, color: Color(okl: 0.78, 0.12, 288), stroke: 2)
                    Text("文字输入给 \(assistantName)")
                        .font(.system(size: 12.5, weight: .semibold)).kerning(0.5)
                        .foregroundColor(Color(okl: 0.78, 0.05, 285))
                }
                .padding(.leading, 4)
                TextField("", text: $draft, prompt: Text("输入消息…").foregroundColor(Color(okl: 0.55, 0.03, 280)), axis: .vertical)
                    .focused($inputFocused)
                    .lineLimit(2...4)
                    .font(.system(size: 15.5))
                    .foregroundColor(Color(okl: 0.95, 0.02, 285))
                    .padding(.horizontal, 14).padding(.vertical, 12)
                    .background(.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 14))
                    .overlay(RoundedRectangle(cornerRadius: 14).stroke(.white.opacity(0.12), lineWidth: 0.5))
                    .onSubmit(submit)
                HStack(spacing: 10) {
                    Spacer()
                    Button("取消") { typing = false; draft = "" }
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color(okl: 0.68, 0.03, 280))
                    Button(action: submit) {
                        HStack(spacing: 6) {
                            Text("发送").font(.system(size: 14, weight: .semibold))
                            LXIcon(name: .arrowUp, size: 15, color: canSend ? .white : Color(okl: 0.58, 0.02, 280))
                        }
                        .foregroundColor(canSend ? .white : Color(okl: 0.58, 0.02, 280))
                        .padding(.horizontal, 18).frame(height: 38)
                        .background {
                            if canSend {
                                LinearGradient(colors: [Color(okl: 0.70, 0.19, 270), Color(okl: 0.66, 0.20, 305)], startPoint: .topLeading, endPoint: .bottomTrailing)
                            } else { Color.white.opacity(0.08) }
                        }
                        .clipShape(RoundedRectangle(cornerRadius: 11))
                    }
                    .disabled(!canSend)
                }
            }
            .padding(14)
            .background(Color(okl: 0.22, 0.02, 275), in: RoundedRectangle(cornerRadius: 22))
            .overlay(RoundedRectangle(cornerRadius: 22).stroke(.white.opacity(0.14), lineWidth: 0.5))
            .shadow(color: .black.opacity(0.5), radius: 25, y: 20)
            .padding(.horizontal, 16).padding(.bottom, 40)
            .transition(.move(edge: .bottom).combined(with: .opacity))
        }
    }

    private var canSend: Bool { !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private func submit() {
        let txt = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !txt.isEmpty else { return }
        typing = false; draft = ""
        submitTurn(txt)
    }
}
