import SwiftUI

/// One in-conversation voice surface shared by ordinary dictation and Flow
/// Mode. It deliberately participates in ChatView's vertical layout so the
/// transcript, composer and keyboard remain usable while voice is active.
struct InlineVoicePanel: View {
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
            .opacity(controller.phase == .configurationRequired ? 0 : 1)
            .allowsHitTesting(controller.phase != .configurationRequired)
            .accessibilityLabel(controller.orbAccessibilityLabel)
            .accessibilityHint(controller.orbAccessibilityHint)

            VStack(spacing: 10) {
                header
                if controller.phase == .configurationRequired {
                    configurationCard
                } else {
                    Spacer(minLength: 52)
                    status
                }
            }
            .frame(maxHeight: .infinity, alignment: .top)
            .padding(14)
            .allowsHitTesting(true)
        }
        .frame(maxWidth: 720)
        .frame(height: controller.phase == .configurationRequired ? 180 : 212)
        .clipShape(.rect(cornerRadius: 22))
        .overlay {
            RoundedRectangle(cornerRadius: 22)
                .stroke(.white.opacity(0.11), lineWidth: 0.5)
        }
        .shadow(color: .black.opacity(0.22), radius: 18, y: 10)
        .shadow(color: theme.accent.tint(0.10), radius: 24, y: 8)
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
        .animation(.spring(response: 0.32, dampingFraction: 0.88), value: controller.phase)
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
        HStack(spacing: 10) {
            Image(systemName: controller.mode == .flow ? "waveform.circle.fill" : "mic.circle.fill")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 30, height: 30)
                .background(
                    LinearGradient(
                        colors: [theme.accent, theme.accent2],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    ),
                    in: Circle()
                )
            Text(controller.mode == .flow ? "voice_flow_mode_label" : "settings_section_voice_input")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color(okl: 0.94, 0.02, 280))
            Spacer()
            if controller.phase == .failed || controller.phase == .paused {
                Button("common_retry", action: controller.retry)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Color(okl: 0.82, 0.12, 285))
                    .padding(.horizontal, 12)
                    .frame(minHeight: 40)
                    .background(.white.opacity(0.07), in: Capsule())
                    .buttonStyle(VoicePanelButtonStyle())
                    .accessibilityIdentifier("voice.retry")
            }
            Button(action: controller.close) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(Color(okl: 0.82, 0.02, 280))
                    .frame(width: 40, height: 40)
                    .background(.white.opacity(0.07), in: Circle())
            }
            .buttonStyle(VoicePanelButtonStyle())
            .accessibilityLabel(controller.mode == .flow ? String(localized: "voice_close_flow_mode_a11y") : String(localized: "voice_close_voice_input_a11y"))
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
            .padding(.horizontal, 11)
            .frame(minHeight: 30)
            .background(.black.opacity(0.20), in: Capsule())
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
        HStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(Color(okl: 0.86, 0.14, 65))
                .frame(width: 42, height: 42)
                .background(Color(okl: 0.75, 0.16, 65, 0.12), in: RoundedRectangle(cornerRadius: 12))

            VStack(alignment: .leading, spacing: 4) {
                Text("voice_config_required_title")
                    .font(.system(size: 13.5, weight: .semibold))
                    .foregroundStyle(Color(okl: 0.96, 0.02, 280))
                    .accessibilityIdentifier("voice.configuration-required")
                Text(controller.statusDetail)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Color(okl: 0.70, 0.03, 280))
                    .lineLimit(3)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Button("settings_title_main", action: onConfigure)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 14)
                .frame(minHeight: 42)
                .background(
                    LinearGradient(
                        colors: [theme.accent, theme.accent2],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    ),
                    in: Capsule()
                )
                .buttonStyle(VoicePanelButtonStyle())
                .accessibilityIdentifier("voice.configure")
        }
        .padding(12)
        .frame(maxWidth: .infinity)
        .background(.black.opacity(0.26), in: RoundedRectangle(cornerRadius: 16))
        .overlay {
            RoundedRectangle(cornerRadius: 16)
                .stroke(.white.opacity(0.08), lineWidth: 0.5)
        }
    }
}

private struct VoicePanelButtonStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.96 : 1)
            .opacity(configuration.isPressed ? 0.84 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}
