import SwiftUI

@main
struct LingxiCodeApp: App {
    @StateObject private var app = AppState()

    init() {
        // M10 A2 / P2 link smoke: a reachable reference to the engine static
        // archive (LingxiCodeFFI.xcframework) so the linker resolves its FFI
        // symbols. Inert at runtime — no engine, no I/O, result discarded.
        // The UI is NOT wired to the engine yet.
        EngineModuleLinkageSmoke.verify()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(app)
                .environment(\.theme, app.palette)
                .preferredColorScheme(app.colorScheme)
                .tint(app.palette.accent)
        }
    }
}
