import SwiftUI

/// One in-conversation voice surface shared by ordinary dictation and Flow
/// Mode. It deliberately participates in ChatView's vertical layout so the
/// transcript, composer and keyboard remain usable while voice is active.
struct InlineVoicePanel: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var theme

    let controller: VoiceInteractionController
    let onConfigure: () -> Void

    var body: some View {
        ZStack {
            panelBackground

            Button(action: controller.handleOrbTap) {
                OrbCanvas(phase: controller.orbPhase, cyFrac: 0.48)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(controller.orbAccessibilityLabel)
            .accessibilityHint(controller.orbAccessibilityHint)

            VStack(spacing: 0) {
                header
                if controller.phase != .configurationRequired {
                    Spacer(minLength: 64)
                    status
                }
            }
            .frame(maxHeight: .infinity, alignment: .top)
            .padding(14)
            .allowsHitTesting(true)

            if controller.phase == .configurationRequired {
                configurationCard
                    .padding(.top, 48)
            }
        }
        .frame(maxWidth: 720)
        .frame(height: 220)
        .clipShape(.rect(cornerRadius: 20))
        .overlay {
            RoundedRectangle(cornerRadius: 20)
                .stroke(.white.opacity(0.12), lineWidth: 0.5)
        }
        .shadow(color: theme.accent.tint(0.12), radius: 20, y: 8)
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conversation.voice-panel")
        .transition(
            .asymmetric(
                insertion: .move(edge: .bottom).combined(with: .opacity),
                removal: .opacity
            )
        )
    }

    private var panelBackground: some View {
        ZStack {
            RadialGradient(
                colors: [Color(okl: 0.20, 0.07, 270), Color(srgb: 0.020, 0.020, 0.039)],
                center: .center,
                startRadius: 0,
                endRadius: 330
            )
            RadialGradient(
                colors: [.clear, .black.opacity(0.42)],
                center: .center,
                startRadius: 60,
                endRadius: 260
            )
        }
        .allowsHitTesting(false)
    }

    private var header: some View {
        HStack(spacing: 9) {
            Image(systemName: controller.mode == .flow ? "waveform.circle.fill" : "mic.circle.fill")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(Color(okl: 0.78, 0.16, 285))
            Text(controller.mode == .flow ? "心流模式" : "语音输入")
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundStyle(Color(okl: 0.94, 0.02, 280))
            Spacer()
            if controller.phase == .failed || controller.phase == .paused {
                Button("重试", action: controller.retry)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Color(okl: 0.82, 0.12, 285))
                    .padding(.horizontal, 10)
                    .frame(minHeight: 32)
                    .background(.white.opacity(0.07), in: Capsule())
                    .accessibilityIdentifier("voice.retry")
            }
            Button(action: controller.close) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(Color(okl: 0.82, 0.02, 280))
                    .frame(width: 32, height: 32)
                    .background(.white.opacity(0.07), in: Circle())
            }
            .accessibilityLabel(controller.mode == .flow ? "关闭心流模式" : "关闭语音输入")
            .accessibilityIdentifier("voice.close")
        }
    }

    private var status: some View {
        VStack(spacing: 5) {
            HStack(spacing: 7) {
                Circle()
                    .fill(controller.statusColor)
                    .frame(width: 7, height: 7)
                    .shadow(color: controller.statusColor, radius: 4)
                Text(controller.statusTitle)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Color(okl: 0.92, 0.02, 280))
            }
            Text(controller.caption.isEmpty ? controller.statusDetail : controller.caption)
                .font(.system(size: 12.5))
                .foregroundStyle(Color(okl: 0.68, 0.03, 280))
                .lineLimit(2)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320, minHeight: 34)
        }
        .allowsHitTesting(false)
    }

    private var configurationCard: some View {
        VStack(spacing: 10) {
            Image(systemName: "exclamationmark.waveform")
                .font(.system(size: 22, weight: .semibold))
                .foregroundStyle(Color(okl: 0.78, 0.16, 60))
            Text("需要配置语音能力")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color(okl: 0.96, 0.02, 280))
            Text(controller.statusDetail)
                .font(.system(size: 11.5))
                .foregroundStyle(Color(okl: 0.70, 0.03, 280))
                .multilineTextAlignment(.center)
                .lineLimit(4)
            Button("去配置", action: onConfigure)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 18)
                .frame(minHeight: 36)
                .background(
                    LinearGradient(
                        colors: [theme.accent, theme.accent2],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    ),
                    in: Capsule()
                )
                .accessibilityIdentifier("voice.configure")
        }
        .padding(.horizontal, 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(srgb: 0.025, 0.022, 0.043).opacity(0.96))
    }
}
