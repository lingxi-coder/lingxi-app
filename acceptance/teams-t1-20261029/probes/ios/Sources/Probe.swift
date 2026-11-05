import SwiftUI

/// Compile-only current Teams tab; the workspace is intentionally not started or signed in.
@main @MainActor struct TeamsMembersProbe: App {
    @State private var workspace = TeamsWorkspace()
    @State private var navigation = AppNavigationModel()
    @State private var tabs = TabCoordinator()
    var body: some Scene {
        WindowGroup { TeamsTab().environment(workspace).environment(navigation).environment(tabs) }
    }
}
