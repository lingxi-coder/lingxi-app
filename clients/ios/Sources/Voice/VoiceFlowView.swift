import SwiftUI

// MARK: - Voice "flow" mode
//
// From the prototype's spec message: "按住屏幕任意位置 0.6 秒会进入沉浸录音态：
// 背景虚化，中央波形脉动，松开立即发送给当前模型。" Background blurs, a central
// waveform pulses, releasing sends. We present this as a full-screen overlay
// that stays up while the finger is held and dismisses (sends) on release.
struct VoiceFlowView: View {
    @Environment(\.theme) private var t
    /// Called on release (the "松开发送" action).
    let onRelease: () -> Void

    @State private var phase: CGFloat = 0
    @State private var appear = false

    var body: some View {
        ZStack {
            // Dimmed, blurred backdrop.
            Rectangle().fill(.ultraThinMaterial).ignoresSafeArea()
            Color.black.opacity(0.35).ignoresSafeArea()

            VStack(spacing: 28) {
                Spacer()
                // Pulsing halo + waveform.
                ZStack {
                    ForEach(0..<3) { i in
                        Circle()
                            .stroke(t.accent.tint(0.4), lineWidth: 1.5)
                            .frame(width: 150 + CGFloat(i) * 60, height: 150 + CGFloat(i) * 60)
                            .scaleEffect(appear ? 1.0 + CGFloat(i) * 0.04 : 0.9)
                            .opacity(appear ? 0.0 : 0.6)
                            .animation(.easeOut(duration: 2).repeatForever(autoreverses: false).delay(Double(i) * 0.4), value: appear)
                    }
                    Circle()
                        .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                        .frame(width: 132, height: 132)
                        .shadow(color: t.accent.tint(0.5), radius: 30)
                    Waveform(phase: phase, color: .white)
                        .frame(width: 70, height: 40)
                }
                Text("正在聆听…").font(.system(size: 17, weight: .semibold)).foregroundColor(t.text)
                Text("松开发送 · 上滑取消").font(.system(size: 13)).foregroundColor(t.text3)
                Spacer()
            }
        }
        .contentShape(Rectangle())
        .transition(.opacity)
        .onAppear {
            appear = true
            withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) { phase = 1 }
        }
    }
}

// Animated audio waveform of vertical bars.
struct Waveform: View {
    var phase: CGFloat
    var color: Color
    private let bars = 7
    var body: some View {
        TimelineView(.animation) { timeline in
            let now = timeline.date.timeIntervalSinceReferenceDate
            HStack(spacing: 5) {
                ForEach(0..<bars, id: \.self) { i in
                    let h = 0.4 + 0.6 * abs(sin(now * 3 + Double(i) * 0.6))
                    Capsule().fill(color).frame(width: 5, height: 40 * h)
                }
            }
        }
    }
}
