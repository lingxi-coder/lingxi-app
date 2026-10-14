import Foundation
import os

/// LingXi's plugin marketplace: the Local App Builder plugin and whatever joins
/// it. The app only registers it, so its plugins are listed under Settings →
/// Plugins for the user to install; the user can remove it like any other.
enum LingxiMarketplace {
    /// The marketplace's own `name`: its plugins are `<plugin>@lingxi`.
    static let name = "lingxi"

    static let githubSource = "lingxi-coder/local-app-builder"

    /// The mirror for mainland China. There is none yet; until then GitHub serves every region.
    static let chinaSource: String? = nil

    /// The source to add the marketplace from, or nil when it should not be
    /// added: it is registered already, or it was once (`seen`) and the user
    /// removed it.
    static func sourceToAdd(_ catalog: PluginCatalog, seen: Bool, providerRegion: String?) -> String? {
        if seen || catalog.marketplace(name) != nil { return nil }
        if providerRegion == "china_mainland" { return chinaSource ?? githubSource }
        return githubSource
    }
}

#if canImport(harness_runtimeFFI)
    /// Registers `LingxiMarketplace` through the plugin system's own
    /// `marketplace_add` once an engine is up, and remembers that it is there
    /// (UserDefaults) so a marketplace the user removed stays removed. It
    /// installs nothing, so the engine needs no reconnect. A failure, such as no
    /// network, is retried on the next launch.
    @MainActor
    final class LingxiMarketplaceSeeder {
        static let shared = LingxiMarketplaceSeeder()

        private let defaults: UserDefaults
        private var running = false
        private static let seenKey = "lingxi.marketplace.seen"
        private static let log = Logger(subsystem: "com.lingxi.code", category: "LingxiMarketplace")

        init(defaults: UserDefaults = .standard) {
            self.defaults = defaults
        }

        func ensure(events: ClientEventCenter, submit: @escaping (ClientCommand) async throws -> Void) async {
            guard !running, !defaults.bool(forKey: Self.seenKey) else { return }
            running = true
            defer { running = false }
            let session = PluginAdminSession(submit: submit)
            let subscription = events.subscribe { [session] event in session.receive(event) }
            defer { events.unsubscribe(subscription) }
            do {
                var catalog = try await session.catalog()
                if let source = LingxiMarketplace.sourceToAdd(catalog, seen: false, providerRegion: await Self.providerRegion()) {
                    guard let revision = catalog.revisions["user"] else { throw SeedError("The plugin catalog has no user revision") }
                    try await session.apply(["action": "marketplace_add", "source": source], revision: revision)
                    catalog = try await session.catalog()
                }
                if catalog.marketplace(LingxiMarketplace.name) != nil { defaults.set(true, forKey: Self.seenKey) }
            } catch is CancellationError {
                return
            } catch {
                Self.log.warning("The LingXi plugin marketplace was not added: \(String(describing: error), privacy: .public)")
            }
        }

        /// The settings' `providerRegion`, once the settings snapshot is in (it
        /// follows the engine connection by a moment), or nil after 10 s.
        private static func providerRegion() async -> String? {
            let settings = DesktopSettingsRepository.shared
            for _ in 0 ..< 100 where !settings.loaded {
                try? await Task.sleep(for: .milliseconds(100))
            }
            return settings.effective["providerRegion"] as? String
        }
    }

    /// The seeder's own line to the engine: plugin admin commands with
    /// operation ids outside the settings pages' range, and the events that
    /// answer them.
    @MainActor
    private final class PluginAdminSession {
        private let submit: (ClientCommand) async throws -> Void
        private var inbox: [ClientEvent] = []

        /// Far above the settings pages' ids, so the two never collide.
        private static var nextOperation: UInt64 = 0x7000_0000_0000_0000

        init(submit: @escaping (ClientCommand) async throws -> Void) {
            self.submit = submit
        }

        func receive(_ event: ClientEvent) {
            switch event {
            case .pluginCatalog, .configurationOperation: inbox.append(event)
            default: break
            }
        }

        /// Asks for the catalog; any catalog that arrives after the request is current.
        func catalog() async throws -> PluginCatalog {
            inbox.removeAll { if case .pluginCatalog = $0 { true } else { false } }
            try await submit(.pluginAdmin(command: .init(
                action: "get_catalog", operationId: nil, target: nil, scope: nil, revision: nil, payloadJson: nil
            )))
            let event = try await next(within: .seconds(30), "The engine did not send the plugin catalog") {
                if case .pluginCatalog = $0 { true } else { false }
            }
            guard case let .pluginCatalog(json) = event else { throw SeedError("Unexpected event") }
            return PluginCatalog.parse(json)
        }

        /// Applies one user-scope operation, confirmed, and waits for it to finish.
        func apply(_ payload: [String: Any], revision: String) async throws {
            Self.nextOperation += 1
            let id = Self.nextOperation
            var body = payload
            body["scope"] = "user"
            body["confirmed"] = true
            try await submit(.pluginAdmin(command: .init(
                action: "apply_operation", operationId: id, target: nil, scope: "user", revision: revision,
                payloadJson: DesktopSettingsRepository.json(body)
            )))
            while true {
                let event = try await next(within: .seconds(120), "The engine did not finish \(payload)") {
                    if case let .configurationOperation(domain, operationId, _, _, _, _) = $0 {
                        domain == .plugin && operationId == id
                    } else { false }
                }
                guard case let .configurationOperation(_, _, status, _, message, _) = event else { continue }
                if status == .failed { throw SeedError("\(payload) failed: \(message ?? "")") }
                if status == .succeeded { return }
            }
        }

        private func next(
            within timeout: Duration,
            _ failure: String,
            matching: (ClientEvent) -> Bool
        ) async throws -> ClientEvent {
            let deadline = ContinuousClock.now + timeout
            while ContinuousClock.now < deadline {
                if let index = inbox.firstIndex(where: matching) { return inbox.remove(at: index) }
                try await Task.sleep(for: .milliseconds(50))
            }
            throw SeedError(failure)
        }
    }

    private struct SeedError: Error, CustomStringConvertible {
        let description: String
        init(_ description: String) { self.description = description }
    }
#endif
