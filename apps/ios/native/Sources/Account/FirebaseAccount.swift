import AuthenticationServices
import CryptoKit
import FirebaseAuth
import FirebaseCore
import GoogleSignIn
import Observation
import UIKit

struct LingxiAccountProfile: Equatable {
    let uid: String
    let email: String?
    let displayName: String?
    let isEmailVerified: Bool
    init(_ user: FirebaseAuth.User) {
        uid = user.uid
        email = user.email
        displayName = user.displayName
        isEmailVerified = user.isEmailVerified
    }
}

@MainActor
@Observable
final class FirebaseAccount {
    static let shared = FirebaseAccount()
    private(set) var user: LingxiAccountProfile?
    private(set) var busy = false
    private(set) var message: String?
    @ObservationIgnored private var listener: AuthStateDidChangeListenerHandle?
    @ObservationIgnored private var appleNonce: String?
    @ObservationIgnored private var appleState: String?

    static func configure() {
        guard FirebaseApp.app() == nil else { return }
        FirebaseApp.configure()
    }
    private init() {
        Self.configure()
        user = Auth.auth().currentUser.map(LingxiAccountProfile.init)
        listener = Auth.auth().addStateDidChangeListener { [weak self] _, user in
            Task { @MainActor in self?.user = user.map(LingxiAccountProfile.init) }
        }
    }
    private func perform(_ operation: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }
        busy = true
        message = nil
        Task { @MainActor in
            defer { busy = false; user = Auth.auth().currentUser.map(LingxiAccountProfile.init) }
            do { try await operation() }
            catch { message = Self.errorMessage(error) }
        }
    }
    func emailSignIn(email: String, password: String, register: Bool) {
        perform {
            let email = email.trimmingCharacters(in: .whitespacesAndNewlines)
            if register { _ = try await Auth.auth().createUser(withEmail: email, password: password) }
            else { _ = try await Auth.auth().signIn(withEmail: email, password: password) }
        }
    }
    func google(presenting controller: UIViewController) {
        perform {
            guard let clientID = FirebaseApp.app()?.options.clientID, controller.view.window != nil else {
                throw AccountError.unavailable
            }
            GIDSignIn.sharedInstance.configuration = GIDConfiguration(clientID: clientID)
            let result = try await GIDSignIn.sharedInstance.signIn(withPresenting: controller)
            guard let token = result.user.idToken?.tokenString else { throw AccountError.unavailable }
            let credential = GoogleAuthProvider.credential(withIDToken: token, accessToken: result.user.accessToken.tokenString)
            _ = try await Auth.auth().signIn(with: credential)
        }
    }
    func prepareApple(_ request: ASAuthorizationAppleIDRequest) {
        var generator = SystemRandomNumberGenerator()
        let nonce = (0..<32).map { _ in String(format: "%02x", UInt8.random(in: .min ... .max, using: &generator)) }.joined()
        let state = UUID().uuidString
        appleNonce = nonce
        appleState = state
        request.requestedScopes = [.fullName, .email]
        request.nonce = SHA256.hash(data: Data(nonce.utf8)).map { String(format: "%02x", $0) }.joined()
        request.state = state
        busy = true
        message = nil
    }
    func completeApple(_ result: Result<ASAuthorization, Error>) {
        let nonce = appleNonce
        let state = appleState
        appleNonce = nil
        appleState = nil
        busy = false
        perform {
            let authorization = try result.get()
            guard let apple = authorization.credential as? ASAuthorizationAppleIDCredential,
                  let nonce, let state, apple.state == state,
                  let data = apple.identityToken, let token = String(data: data, encoding: .utf8) else {
                throw AccountError.unavailable
            }
            let credential = OAuthProvider.appleCredential(withIDToken: token, rawNonce: nonce, fullName: apple.fullName)
            _ = try await Auth.auth().signIn(with: credential)
        }
    }
    func resetPassword(email: String) {
        perform {
            try await Auth.auth().sendPasswordReset(withEmail: email.trimmingCharacters(in: .whitespacesAndNewlines))
            self.message = "如果此邮箱已注册，将收到密码重置邮件。"
        }
    }
    func verifyEmail() {
        perform {
            guard let user = Auth.auth().currentUser else { throw AccountError.unavailable }
            try await user.sendEmailVerification()
            self.message = "验证邮件已发送，请检查收件箱。"
        }
    }
    func reload() {
        guard user != nil else { return }
        perform { try await Auth.auth().currentUser?.reload() }
    }
    func signOut() {
        perform {
            try Auth.auth().signOut()
            GIDSignIn.sharedInstance.signOut()
        }
    }
    private enum AccountError: Error { case unavailable }
    private static func errorMessage(_ error: Error) -> String {
        let error = error as NSError
        if error.domain == ASAuthorizationError.errorDomain && error.code == ASAuthorizationError.canceled.rawValue { return "登录已取消。" }
        if error.domain == kGIDSignInErrorDomain && error.code == GIDSignInError.canceled.rawValue { return "登录已取消。" }
        guard error.domain == AuthErrorDomain else { return "操作未完成，请检查网络或登录服务配置后重试。" }
        switch error.code {
        case AuthErrorCode.wrongPassword.rawValue, AuthErrorCode.invalidCredential.rawValue, AuthErrorCode.userNotFound.rawValue: return "邮箱或密码不正确。"
        case AuthErrorCode.emailAlreadyInUse.rawValue: return "此邮箱已注册，请登录或重置密码。"
        case AuthErrorCode.weakPassword.rawValue: return "密码强度不足，请使用更长的密码。"
        case AuthErrorCode.operationNotAllowed.rawValue: return "此登录方式尚未启用。"
        case AuthErrorCode.accountExistsWithDifferentCredential.rawValue: return "请先使用原有登录方式登录此邮箱。"
        case AuthErrorCode.userDisabled.rawValue: return "此账号已停用。"
        default: return "操作未完成，请检查网络后重试。"
        }
    }
}
