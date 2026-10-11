import AuthenticationServices
import SwiftUI

struct LingxiAccountSection: View {
    @State private var account = FirebaseAccount.shared
    @State private var email = ""
    @State private var password = ""
    @State private var register = false
    @FocusState private var passwordFocused: Bool
    @State private var presenter: UIViewController?
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("LingXi 账户").font(.title2.bold())
            if let user = account.user {
                Text(user.displayName ?? user.email ?? "已登录").font(.headline).accessibilityIdentifier("account.profile")
                if let email = user.email { Text(email).foregroundStyle(.secondary) }
                Text(user.isEmailVerified ? "邮箱已验证" : "邮箱尚未验证").font(.footnote).foregroundStyle(.secondary)
                if !user.isEmailVerified && user.email != nil { Button("发送验证邮件", action: account.verifyEmail) }
                Button("刷新账户", action: account.reload)
                Button("退出登录", action: account.signOut).accessibilityIdentifier("account.signout")
            } else {
                TextField("邮箱", text: $email).accessibilityIdentifier("account.email")
                    .textContentType(.username).keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("密码", text: $password).textContentType(register ? .newPassword : .password)
                    .accessibilityIdentifier("account.password")
                    .focused($passwordFocused)
                    .submitLabel(.go)
                    .onSubmit { passwordFocused = false; if !email.isEmpty && !password.isEmpty { account.emailSignIn(email: email, password: password, register: register) } }
                Button(register ? "创建账户" : "登录") {
                    passwordFocused = false
                    account.emailSignIn(email: email, password: password, register: register)
                }
                    .accessibilityIdentifier("account.submit").buttonStyle(.borderedProminent).disabled(email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || password.isEmpty)
                Button(register ? "已有账户，登录" : "注册新账户") { register.toggle(); password = "" }.accessibilityIdentifier("account.mode")
                Button("忘记密码") { account.resetPassword(email: email) }.disabled(email.isEmpty)
                Button("通过 Google 登录") { if let presenter { account.google(presenting: presenter) } }
                    .buttonStyle(.bordered).disabled(presenter == nil).accessibilityIdentifier("account.google")
                SignInWithAppleButton(.signIn, onRequest: account.prepareApple, onCompletion: account.completeApple)
                    .signInWithAppleButtonStyle(colorScheme == .dark ? .white : .black)
                    .frame(height: 44).accessibilityIdentifier("account.apple")
            }
            if account.busy { ProgressView("正在处理…") }
            if let message = account.message { Text(message).accessibilityIdentifier("account.message").font(.footnote).accessibilityAddTraits(.updatesFrequently) }
            Divider()
        }
        .textFieldStyle(.roundedBorder)
        .disabled(account.busy)
        .background(AccountPresentationAnchor { presenter = $0 }.frame(width: 0, height: 0))
        .onChange(of: account.user?.uid) { password = "" }
        .onChange(of: scenePhase) { _, phase in if phase == .active { account.reload() } }
    }
}

private struct AccountPresentationAnchor: UIViewControllerRepresentable {
    let resolve: (UIViewController) -> Void
    func makeUIViewController(context: Context) -> UIViewController {
        let controller = UIViewController()
        DispatchQueue.main.async { resolve(controller) }
        return controller
    }
    func updateUIViewController(_ controller: UIViewController, context: Context) {}
}
