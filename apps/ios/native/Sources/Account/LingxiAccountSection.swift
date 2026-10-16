import SwiftUI

/// Account authentication owns its native form; model-provider credentials live
/// in Models & Services and are intentionally outside this page.
struct LingxiAccountSection: View {
    @State private var account = FirebaseAccount.shared
    @State private var email = ""
    @State private var password = ""
    @State private var register = false
    @FocusState private var passwordFocused: Bool
    @State private var presenter: UIViewController?
    @Environment(\.scenePhase) private var scenePhase

    private var canSubmit: Bool {
        !email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !password.isEmpty
    }

    var body: some View {
        Form {
            Section {
                VStack(spacing: 12) {
                    Image(systemName: "person.crop.circle.fill")
                        .font(.system(size: 56)).foregroundStyle(.secondary)
                        .accessibilityHidden(true)
                    Text("account_title").font(.title2.bold())
                    if let user = account.user {
                        Text(user.displayName ?? user.email ?? String(localized: "account_signed_in"))
                            .font(.headline).textSelection(.enabled)
                            .accessibilityIdentifier("account.profile")
                    } else {
                        Text("account_sign_in_subtitle")
                            .font(.subheadline).foregroundStyle(.secondary)
                    }
                }
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
            }
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)

            if let user = account.user {
                Section {
                    if let email = user.email {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("account_email").font(.subheadline).foregroundStyle(.secondary)
                            Text(email).textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityIdentifier("account.email.value")
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    Label(user.isEmailVerified ? "account_email_verified" : "account_email_unverified",
                          systemImage: user.isEmailVerified ? "checkmark.seal.fill" : "exclamationmark.circle")
                        .foregroundStyle(user.isEmailVerified ? Color.green : Color.secondary)
                    if !user.isEmailVerified && user.email != nil {
                        Button("account_send_verification", action: account.verifyEmail)
                    }
                    Button(action: account.reload) {
                        Label("account_refresh", systemImage: "arrow.clockwise")
                    }
                    .accessibilityIdentifier("account.refresh")
                }
                Section {
                    Button("account_sign_out", role: .destructive, action: account.signOut)
                        .accessibilityIdentifier("account.signout")
                }
            } else {
                Section {
                    Button {
                        passwordFocused = false
                        if let presenter { account.google(presenting: presenter) }
                    } label: {
                        HStack(spacing: 12) {
                            Image("GoogleG").renderingMode(.original).resizable()
                                .frame(width: 20, height: 20).accessibilityHidden(true)
                            Text("account_google").font(.body.weight(.medium))
                        }
                        .foregroundStyle(Color(red: 31 / 255, green: 31 / 255, blue: 31 / 255))
                        .padding(.horizontal, 16).padding(.vertical, 12)
                        .frame(maxWidth: .infinity, minHeight: 50)
                        .background(.white, in: Capsule())
                        .overlay(Capsule().strokeBorder(Color(uiColor: .systemGray)))
                        .contentShape(Capsule())
                    }
                    .buttonStyle(.plain).disabled(presenter == nil)
                    .accessibilityIdentifier("account.google")
                    .listRowInsets(EdgeInsets())
                }
                .listRowBackground(Color.clear)

                Section {
                    TextField("account_email", text: $email)
                        .accessibilityIdentifier("account.email")
                        .textContentType(.username).keyboardType(.emailAddress)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                    SecureField("account_password", text: $password)
                        .textContentType(register ? .newPassword : .password)
                        .accessibilityIdentifier("account.password")
                        .focused($passwordFocused).submitLabel(.go)
                        .onSubmit { if canSubmit { submit() } }
                } header: {
                    Text(register ? "account_email_register" : "account_email_sign_in")
                }
                .textFieldStyle(.plain)

                Section {
                    Button(action: submit) {
                        Text(register ? "account_create" : "account_sign_in")
                            .font(.body.weight(.semibold))
                            .frame(maxWidth: .infinity, minHeight: 28)
                    }
                    .buttonStyle(.borderedProminent).controlSize(.large)
                    .disabled(!canSubmit).accessibilityIdentifier("account.submit")
                    .listRowInsets(EdgeInsets())
                }
                .listRowBackground(Color.clear)

                Section {
                    Button(register ? "account_existing" : "account_register") {
                        passwordFocused = false
                        register.toggle()
                        password = ""
                    }
                    .accessibilityIdentifier("account.mode")
                    Button("account_forgot_password") { account.resetPassword(email: email) }
                        .disabled(email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }

            if account.busy || account.message != nil {
                Section {
                    if account.busy { ProgressView("account_processing") }
                    if let message = account.message {
                        Text(message).font(.subheadline).foregroundStyle(.secondary)
                            .accessibilityIdentifier("account.message")
                            .accessibilityAddTraits(.updatesFrequently)
                    }
                }
            }
            Section {
                Link(destination: URL(string: "https://lingxios.net/privacy")!) {
                    Label("privacy_policy_link", systemImage: "hand.raised")
                }
            } footer: {
                Text("account_local_use_note")
            }
        }
        .formStyle(.grouped)
        .scrollDismissesKeyboard(.interactively)
        .disabled(account.busy)
        .background(AccountPresentationAnchor { presenter = $0 }.frame(width: 0, height: 0))
        .onChange(of: account.user?.uid) { password = "" }
        .onChange(of: scenePhase) { _, phase in if phase == .active { account.reload() } }
    }

    private func submit() {
        passwordFocused = false
        account.emailSignIn(email: email, password: password, register: register)
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
