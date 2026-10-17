import SwiftUI
import GoogleSignIn

@main
struct LingxiCodeApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @UIApplicationDelegateAdaptor(AppNotificationDelegate.self) private var appDelegate
    @State private var app = AppState()
    @State private var voice = VoiceCapabilityModel()

    init() {
        FirebaseAccount.configure()
        LXISHRuntimeResourceBootstrap.prepare()
        // M10 A2 / P2 link smoke: a reachable reference to the engine static
        // archive (LingxiCodeFFI.xcframework) so the linker resolves its FFI
        // symbols. Inert at runtime — no engine, no I/O, result discarded.
        // The conversation UI itself is wired to the engine via
        // `ConversationSourceFactory.make()` (EngineConversationSource over
        // UniFFI when the bindings are linked + opted in; the canned mock
        // otherwise) — this call only proves the FFI archive links.
        EngineModuleLinkageSmoke.verify()
    }

    var body: some Scene {
        WindowGroup {
            RootView(voiceCapability: voice)
                .onOpenURL { _ = GIDSignIn.sharedInstance.handle($0) }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active { PushRegistration.shared.refresh() }
                }
                .environment(app)
                .environment(voice)
                .environment(LocalizationManager.shared)
                .environment(\.theme, app.palette)
                .modifier(NativeAppearancePreferences(app: app))
                .preferredColorScheme(app.colorScheme)
                .tint(app.palette.accent)
        }
    }
}
