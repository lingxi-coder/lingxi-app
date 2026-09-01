import SwiftUI
import WidgetKit

@main
struct LocalAppsWidgetBundle: WidgetBundle {
    var body: some Widget {
        LocalAppWidget()
        #if canImport(ActivityKit)
            if #available(iOS 18.0, *) {
                ConversationLiveActivityWidget()
            }
        #endif
    }
}
