import Foundation
import SwiftUI

/// Only recovery copy reaches the composer; raw provider errors can contain secrets.
enum AudioSetupGuidance: Equatable {
    case permission, quota, configuration, retry

    static func fromError(_ message: String) -> Self? {
        let error = message.lowercased()
        if error.contains("cancelled") || error.contains("canceled") { return nil }
        if error.contains("permission") || error.contains("denied") || error.contains("权限") || error.contains("權限") { return .permission }
        if error.contains("429") || error.contains("quota") { return .quota }
        if ["unsupported", "not supported", "unavailable", "model", "configuration", "language", "systemunavailable", "模型", "配置", "语言", "語言"]
            .contains(where: error.contains) { return .configuration }
        return .retry
    }

    var title: String {
        switch self {
        case .permission: String(localized: "audio_guide_permission_title")
        case .quota: String(localized: "audio_guide_quota_title")
        case .configuration: String(localized: "audio_guide_configuration_title")
        case .retry: String(localized: "audio_guide_retry_title")
        }
    }

    var detail: String {
        switch self {
        case .permission: String(localized: "audio_guide_permission_detail")
        case .quota: String(localized: "audio_guide_quota_detail")
        case .configuration: String(localized: "audio_guide_configuration_detail")
        case .retry: String(localized: "audio_guide_retry_detail")
        }
    }

    static func forPhase(_ phase: VoiceInteractionPhase, message: String) -> Self? {
        switch phase {
        case .configurationRequired: fromError(message) == .permission ? .permission : .configuration
        case .failed: fromError(message)
        default: nil
        }
    }
}

struct AudioSetupBanner: View {
    @Environment(\.theme) private var theme
    let guidance: AudioSetupGuidance
    let onConfigure: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(guidance.title, systemImage: "mic")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(theme.text)
            Text(guidance.detail)
                .font(.footnote)
                .foregroundStyle(theme.text2)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 14) {
                Spacer()
                Button("audio_guide_dismiss", action: onDismiss)
                    .foregroundStyle(theme.text3)
                    .frame(minWidth: 44, minHeight: 44)
                    .contentShape(Rectangle())
                    .accessibilityIdentifier("conversation.audio-setup.dismiss")
                Button("audio_guide_settings", action: onConfigure)
                    .foregroundStyle(theme.accent)
                    .frame(minWidth: 44, minHeight: 44)
                    .contentShape(Rectangle())
                    .accessibilityIdentifier("conversation.audio-setup.configure")
            }
            .font(.footnote.weight(.medium))
            .buttonStyle(.plain)
            .frame(minHeight: 44)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(theme.accent.opacity(0.25), lineWidth: 0.5))
        .padding(.horizontal, 14)
        .padding(.bottom, 6)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conversation.audio-setup")
    }
}
