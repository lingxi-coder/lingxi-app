import FirebaseCore
import Observation
import SwiftUI

// Only the shell is substituted. All Teams UI, logic, storage and SDK models are production sources.
// Fixture apps configure their own demo Firebase instance before starting a workspace.
enum FirebaseAccount { static func configure() {} }
enum SettingsPage { case account }
@MainActor @Observable final class AppNavigationModel {
    func showSettings(_ page: SettingsPage, origin: AppTab?) {}
}
struct ProbePalette { var isDark = false }
private struct ProbeThemeKey: EnvironmentKey { static let defaultValue = ProbePalette() }
extension EnvironmentValues { var theme: ProbePalette { self[ProbeThemeKey.self] } }
extension View { func hidesTabBarWhenPushed(in tab: AppTab) -> some View { self } }
