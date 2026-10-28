import SwiftUI

@main
struct SettingsAXProbeApp: App {
    var body: some Scene {
        WindowGroup { Root() }
    }
}

private struct Root: View {
    @State private var settings = false
    @State private var composer = ""

    var body: some View {
        VStack {
            Button("设置") { settings = true }
                .accessibilityIdentifier("drawer.settings")
            TextField("输入", text: $composer)
                .accessibilityIdentifier("composer.input")
        }
        .sheet(isPresented: $settings) {
            SettingsProbe(compactClose: ProcessInfo.processInfo.arguments.contains("--compact-close")) {
                settings = false
            }
        }
    }
}

private enum Page: Hashable { case account, models }

private struct SettingsProbe: View {
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var path: [Page] = []
    @State private var search = ""
    let compactClose: Bool
    let close: () -> Void

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Button("LingXi 账户") { path.append(.account) }
                    .accessibilityIdentifier("settings.page.account")
                Button("模型") { path.append(.models) }
                    .accessibilityIdentifier("settings.group.models")
            }
            .listStyle(.insetGrouped)
            .searchable(text: $search, placement: .navigationBarDrawer(displayMode: .always), prompt: "搜索设置")
            .navigationTitle("设置")
            .navigationBarTitleDisplayMode(.large)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("完成", action: close).accessibilityIdentifier("settings.close")
                }
            }
            .navigationDestination(for: Page.self) { page in
                pageContent(page)
                    .navigationTitle(page == .account ? "LingXi 账户" : "模型")
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .topBarTrailing) {
                            Button(action: close) {
                                if compactClose && typeSize.isAccessibilitySize {
                                    Image(systemName: "xmark")
                                        .font(.system(size: 17, weight: .semibold))
                                        .frame(minWidth: 44, minHeight: 44)
                                } else {
                                    Text("完成").font(.subheadline.weight(.semibold))
                                }
                            }
                            .accessibilityLabel(compactClose && typeSize.isAccessibilitySize ? "关闭设置" : "完成")
                            .accessibilityIdentifier("settings.done")
                        }
                    }
            }
        }
    }

    @ViewBuilder
    private func pageContent(_ page: Page) -> some View {
        switch page {
        case .account: AccountForm()
        case .models: ModelsHome()
        }
    }
}

private struct AccountForm: View {
    @State private var email = ""
    @State private var password = ""
    @FocusState private var passwordFocused: Bool

    var body: some View {
        Form {
            Section {
                VStack(spacing: 12) {
                    Image(systemName: "person.crop.circle.fill").font(.system(size: 56))
                    Text("LingXi 账户").font(.title2.bold())
                    Text("登录灵犀账户，同步与账户设置").font(.subheadline)
                }
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
            }
            .listRowBackground(Color.clear)
            Section {
                Button("使用 Google 登录") {}.frame(minHeight: 50)
                    .accessibilityIdentifier("account.google")
            }
            Section {
                TextField("电子邮箱", text: $email)
                    .textContentType(.username).keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    .accessibilityIdentifier("account.email")
                SecureField("密码", text: $password)
                    .textContentType(.password).focused($passwordFocused).submitLabel(.go)
                    .accessibilityIdentifier("account.password")
            } header: { Text("邮箱登录") }
            Section {
                Button("登录") {}.disabled(email.isEmpty || password.isEmpty)
                    .accessibilityIdentifier("account.submit")
            }
            Section { Button("注册新账户") {}; Button("忘记密码") {} }
        }
        .formStyle(.grouped)
        .scrollDismissesKeyboard(.interactively)
    }
}

private struct ModelsHome: View {
    @State private var add = false
    var body: some View {
        List { Button("添加服务商") { add = true }.accessibilityIdentifier("provider.add") }
            .sheet(isPresented: $add) { ProviderAddProbe() }
    }
}

private struct ProviderAddProbe: View {
    @State private var query = ""
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        NavigationStack {
            List {
                NavigationLink("DeepSeek") { ProviderEditorProbe() }
                    .accessibilityIdentifier("provider.preset.deepseek")
            }
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "搜索设置")
            .navigationTitle("添加服务商")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }.accessibilityIdentifier("provider.cancel")
                }
            }
        }
    }
}

private struct ProviderEditorProbe: View {
    var body: some View {
        Form {
            NavigationLink("模型") { ProviderModelsProbe() }
                .accessibilityIdentifier("provider.model")
        }
        .navigationTitle("DeepSeek")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) { Button("保存") {} }
        }
    }
}

private struct ProviderModelsProbe: View {
    @State private var visible = true
    @State private var search = ""
    @State private var visibilitySearch = ""
    var body: some View {
        List {
            NavigationLink {
                List {
                    Toggle("在模型选择器中显示", isOn: $visible)
                        .accessibilityIdentifier("provider.model.visible")
                }
                .listStyle(.insetGrouped)
                .searchable(text: $visibilitySearch, prompt: "搜索模型")
                .navigationTitle("显示的模型")
                .navigationBarTitleDisplayMode(.inline)
            } label: { LabeledContent("显示的模型", value: visible ? "1 / 1" : "已禁用") }
                .accessibilityIdentifier("provider.model.visibility")
        }
        .listStyle(.insetGrouped)
        .searchable(text: $search, placement: .navigationBarDrawer(displayMode: .always), prompt: "搜索模型")
        .navigationTitle("模型")
        .navigationBarTitleDisplayMode(.inline)
    }
}
