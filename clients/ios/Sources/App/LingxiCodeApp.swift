import SwiftUI

@main
struct LingxiCodeApp: App {
    @StateObject private var app = AppState()

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
