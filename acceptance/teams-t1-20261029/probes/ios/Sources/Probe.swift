import SwiftUI

/// Compiles the actual product membership page/model/SDK; never contacts a real
/// account during the unsigned build. Product packaging is a separate gate.
@main struct TeamsMembersProbe: App {
    var body: some Scene {
        WindowGroup { NavigationStack { TeamsPage(initialInvite: nil, onOpenAccount: {}) } }
    }
}
