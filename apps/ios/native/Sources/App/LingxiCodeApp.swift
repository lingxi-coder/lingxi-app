import SwiftUI
import GoogleSignIn

@main
struct LingxiCodeApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @UIApplicationDelegateAdaptor(AppNotificationDelegate.self) private var appDelegate
    @State private var app = AppState()
    @State private var voice = VoiceCapabilityModel()
    #if canImport(harness_runtimeFFI)
    @State private var mediaHostComposition: IOSNativeMediaHostComposition?
    @State private var mediaHostInitializationError: String?
    #endif

    init() {
        FirebaseAccount.configure()
        LXISHRuntimeResourceBootstrap.prepare()
        #if canImport(harness_runtimeFFI)
        let mediaHostComposition: IOSNativeMediaHostComposition?
        let mediaHostInitializationError: String?
        do {
            guard let audioConfigurationJSON = AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON else {
                throw NSError(
                    domain: "LingxiMediaHostComposition",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "The saved v4 audio settings could not be encoded for the native media host."]
                )
            }
            mediaHostComposition = try IOSNativeMediaHostComposition(
                audioConfigurationJSON: audioConfigurationJSON
            )
            mediaHostInitializationError = nil
        } catch {
            mediaHostComposition = nil
            mediaHostInitializationError = "The native media host could not be initialized: \(error.localizedDescription)"
        }
        _mediaHostComposition = State(initialValue: mediaHostComposition)
        _mediaHostInitializationError = State(initialValue: mediaHostInitializationError)
        #endif
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
            Group {
                #if canImport(harness_runtimeFFI)
                RootView(
                    voiceCapability: voice,
                    mediaHostComposition: mediaHostComposition,
                    mediaHostInitializationError: mediaHostInitializationError
                )
                #else
                RootView(voiceCapability: voice)
                #endif
            }
                .onOpenURL { _ = GIDSignIn.sharedInstance.handle($0) }
                .onChange(of: scenePhase) { _, phase in
                    if phase == ScenePhase.active { PushRegistration.shared.refresh() }
                }
                .environment(app)
                .environment(voice)
                .environment(LocalizationManager.shared)
                .environment(\EnvironmentValues.theme, app.palette)
                .modifier(NativeAppearancePreferences(app: app))
                .preferredColorScheme(app.colorScheme)
                .tint(app.palette.accent)
        }
    }
}
