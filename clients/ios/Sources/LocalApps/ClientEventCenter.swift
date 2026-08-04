import Foundation

@MainActor
final class ClientEventCenter {
    struct Subscription: Hashable {
        fileprivate let id: UUID
    }

    #if canImport(engine_mobileFFI)
        private var handlers: [UUID: (ClientEvent) -> Void] = [:]

        @discardableResult
        func subscribe(_ handler: @escaping (ClientEvent) -> Void) -> Subscription {
            let subscription = Subscription(id: UUID())
            handlers[subscription.id] = handler
            return subscription
        }

        func unsubscribe(_ subscription: Subscription) {
            handlers[subscription.id] = nil
        }

        func publish(_ event: ClientEvent) {
            let snapshot = Array(handlers.values)
            for handler in snapshot { handler(event) }
        }
    #endif
}
