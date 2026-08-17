import SwiftUI

struct PermissionModePage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    @State private var pending: String?

    private let options = [
        RadioOption(value: "default", label: "询问（Default）", sub: "需要授权的操作显示确认。"),
        RadioOption(value: "acceptEdits", label: "自动接受编辑", sub: "自动允许安全的文件编辑。"),
        RadioOption(value: "plan", label: "计划模式", sub: "允许分析和读取，修改操作需要确认。"),
        RadioOption(value: "auto", label: "自动（Auto）", sub: "由模型/Provider 安全门控决定是否自动放行。"),
        RadioOption(value: "dontAsk", label: "不询问（Dont Ask）", sub: "不弹窗；未明确允许的操作直接拒绝。"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSection(label: "权限模式", footer: "自动模式会按当前模型和策略安全降级；当前选择会随会话保存。完整访问仅能从会话控制中确认启用。") {
                RadioList(options: options, value: Binding(
                    get: { store.permissionMode },
                    set: { mode in
                        if mode == "dontAsk" || mode == "bypassPermissions" {
                            pending = mode
                        } else {
                            host.applyPermissionMode(mode)
                        }
                    }
                ))
            }
            if store.effectivePermissionMode != store.permissionMode {
                Text("当前有效模式：\(store.effectivePermissionMode)")
                    .font(.system(size: 12)).foregroundColor(.secondary).padding(.horizontal, 4)
            }
            if let error = store.permissionModeError, !error.isEmpty {
                Text(error)
                    .font(.system(size: 12)).foregroundColor(.red).padding(.horizontal, 4)
            }
        }
        .alert("确认高风险权限模式", isPresented: Binding(
            get: { pending != nil },
            set: { if !$0 { pending = nil } }
        )) {
            Button("确认应用", role: .destructive) {
                if let mode = pending { host.applyPermissionMode(mode) }
                pending = nil
            }
            Button("取消", role: .cancel) { pending = nil }
        } message: {
            Text("该模式可能允许工具执行高风险操作。确认后才会应用。")
        }
    }
}
