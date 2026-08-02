import SwiftUI

@main
struct LingxiCodeApp: App {
    @UIApplicationDelegateAdaptor(AppNotificationDelegate.self) private var appDelegate
    @State private var app = AppState()

    init() {
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
            RootView()
                .environment(app)
                .environment(\.theme, app.palette)
                .preferredColorScheme(app.colorScheme)
                .tint(app.palette.accent)
        }
    }
}
