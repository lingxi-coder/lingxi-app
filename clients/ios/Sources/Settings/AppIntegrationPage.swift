import SwiftUI

struct AppIntegrationPage: View {
    @Environment(\.openURL) private var openURL
    @Environment(\.theme) private var t

    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(
                label: "系统接入",
                footer: "这些动作由 iOS App Intents 提供，可在 Siri、快捷指令和个人自动化中直接使用。"
            ) {
                capabilityRow(
                    icon: .sparkle,
                    title: "打开灵犀",
                    detail: "回到当前项目与会话"
                )
                capabilityRow(
                    icon: .edit,
                    title: "新建对话",
                    detail: "在当前项目创建空白对话"
                )
                capabilityRow(
                    icon: .message,
                    title: "向灵犀提问",
                    detail: "接收 Siri 或其他 App 传入的文本"
                )
                capabilityRow(
                    icon: .workflow,
                    title: "打开终端",
                    detail: "支持快捷指令与 lingxi://open_terminal 深链",
                    isLast: true
                )
            }

            SettingsSection(
                label: "快捷指令",
                footer: "可在系统快捷指令中组合分享表单、剪贴板、文件或其他 App 的输出，再交给灵犀。"
            ) {
                SettingsRow(
                    icon: .workflow,
                    iconColor: t.accent,
                    label: "打开快捷指令",
                    sub: "查看灵犀提供的动作",
                    value: "4 个动作",
                    isLast: true,
                    onTap: openShortcuts
                )
                .accessibilityIdentifier("settings.appIntegration.openShortcuts")
            }

            SettingsSection(label: "平台边界") {
                VStack(alignment: .leading, spacing: 8) {
                    Label("使用 iOS 公共 API，动作会由系统展示和授权。", systemImage: "checkmark.shield")
                    Label("不会读取、点击或控制其他 App 的界面。", systemImage: "hand.raised")
                    Label("跨 App 数据通过快捷指令输入或系统分享完成。", systemImage: "square.and.arrow.up")
                }
                .font(.system(size: 12.5))
                .foregroundStyle(t.text2)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func capabilityRow(
        icon: LXIconName,
        title: String,
        detail: String,
        isLast: Bool = false
    ) -> some View {
        SettingsRow(
            icon: icon,
            iconColor: t.accent,
            label: title,
            sub: detail,
            value: "可用",
            valueColor: t.ok,
            chevron: false,
            isLast: isLast
        )
    }

    private func openShortcuts() {
        guard let url = URL(string: "shortcuts://") else { return }
        openURL(url)
    }
}
