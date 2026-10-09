import SwiftUI
import WidgetKit

@main
struct LocalAppsWidgetBundle: WidgetBundle {
    var body: some Widget {
        #if canImport(ActivityKit)
            if #available(iOS 18.0, *) {
                ConversationLiveActivityWidget()
            }
        #endif
    }
}
