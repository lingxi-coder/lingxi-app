import SwiftUI
import WidgetKit

@main
struct ConversationWidgetBundle: WidgetBundle {
    var body: some Widget {
        #if canImport(ActivityKit)
            if #available(iOS 18.0, *) {
                ConversationLiveActivityWidget()
            }
        #endif
    }
}
