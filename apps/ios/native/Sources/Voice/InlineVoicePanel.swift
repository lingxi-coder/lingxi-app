import SwiftUI

/// One in-conversation voice surface shared by ordinary dictation and Flow
/// Mode. It deliberately participates in ChatView's vertical layout so the
/// transcript, composer and keyboard remain usable while voice is active.
struct InlineVoicePanel: View {
    @Environment(\.theme) private var theme

    @ScaledMetric(relativeTo: .body) private var panelHeight = 212
    @ScaledMetric(relativeTo: .body) private var configurationHeight = 180

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
        .frame(height: controller.phase == .configurationRequired ? configurationHeight : panelHeight)
        .clipShape(.rect(cornerRadius: 22))
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
        .animation(.spring(response: 0.32, dampingFraction: 0.88), value: controller.phase)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conversation.voice-panel")
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }

    private var panelBackground: some View {
        theme.surface.allowsHitTesting(false)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: controller.mode == .flow ? "waveform.circle.fill" : "mic.circle.fill")
                .font(.subheadline.weight(.semibold))
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
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(theme.text)
            Spacer()
            if controller.phase == .failed || controller.phase == .paused {
                Button("common_retry", action: controller.retry)
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(theme.accent)
                    .padding(.horizontal, 12)
                    .frame(minHeight: 44)
                    .background(theme.surfaceActive, in: Capsule())
                    .buttonStyle(VoicePanelButtonStyle())
                    .accessibilityIdentifier("voice.retry")
            }
            Button(action: controller.close) {
                Image(systemName: "xmark")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(theme.text2)
                    .frame(width: 44, height: 44)
                    .background(theme.surfaceActive, in: Circle())
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
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(theme.text)
            }
            .padding(.horizontal, 11)
            .frame(minHeight: 30)
            .background(theme.surface, in: Capsule())
            Text(controller.caption.isEmpty ? controller.statusDetail : controller.caption)
                .font(.footnote)
                .foregroundStyle(theme.text2)
                .lineLimit(2)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320, minHeight: 34)
        }
        .allowsHitTesting(false)
    }

    private var configurationCard: some View {
        HStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.body.weight(.semibold))
                .foregroundStyle(theme.statusTesting)
                .frame(width: 42, height: 42)
                .background(theme.statusTesting.opacity(0.12), in: RoundedRectangle(cornerRadius: 12))

            VStack(alignment: .leading, spacing: 4) {
                Text("voice_config_required_title")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(theme.text)
                    .accessibilityIdentifier("voice.configuration-required")
                Text(controller.statusDetail)
                    .font(.caption)
                    .foregroundStyle(theme.text2)
                    .lineLimit(3)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Button("settings_title_main", action: onConfigure)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
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
        .background(theme.surfaceHover, in: RoundedRectangle(cornerRadius: 16))
        .overlay {
            RoundedRectangle(cornerRadius: 16)
                .stroke(theme.border, lineWidth: 0.5)
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
