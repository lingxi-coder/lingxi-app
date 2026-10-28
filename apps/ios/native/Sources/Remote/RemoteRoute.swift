import Foundation

#if canImport(harness_runtimeFFI)
/// The screens of Remote Control, as the navigation stack holds them.
enum RemoteRoute: Hashable {
    case host(String)
    case bot(host: String, bot: String)
    case session(host: String, session: String, title: String)

    /// The stack that shows a computer, and a chat of it when there is one. A chat that belongs to a Bot
    /// has the Bot between the computer and itself, so that back from the chat returns to the Bot; one
    /// whose Bot is not known (or that has none) goes back to the computer.
    static func stack(host: String, session: String?, title: String, bot: String?) -> [RemoteRoute] {
        var routes: [RemoteRoute] = [.host(host)]
        guard let session else { return routes }
        if let bot { routes.append(.bot(host: host, bot: bot)) }
        routes.append(.session(host: host, session: session, title: title))
        return routes
    }
}
#endif
