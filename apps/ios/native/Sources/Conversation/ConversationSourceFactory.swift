import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif


/// Chooses the conversation source at app start. Prefers the real in-process
/// engine (over UniFFI) when the bindings are linked AND the engine is opted in;
/// otherwise the canned mock. Falling back to the mock keeps the app usable in
/// preview / no-key environments.
///
/// With the FFI linked, shipped builds always use the real in-process engine —
/// even keyless — so missing provider credentials surface as real engine errors
/// rather than silently falling back to canned data. The mock remains only for
/// preview / no-FFI environments.
@MainActor
enum ConversationSourceFactory {
    struct LaunchOptions {
        var projectCwd: String? = nil
        var sessionMode: SessionMode = .code
        var providerConfigured: Bool = false
        var providerProfilesJson: String? = nil
        var providerRoutingJson: String? = nil
        var defaultModelID: String? = nil
        var visionDelegationEnabled: Bool = true
        var mobileLinux: TerminalRuntimeConfig? = nil
    }

    static func make(
        projectCwd: String? = nil,
        sessionMode: SessionMode = .code,
        providerConfigured: Bool = false,
        providerProfilesJson: String? = nil,
        providerRoutingJson: String? = nil,
        defaultModelID: String? = nil,
        visionDelegationEnabled: Bool = true,
        mobileLinux: TerminalRuntimeConfig? = nil
    ) -> any ConversationSource {
        make(options: LaunchOptions(
            projectCwd: projectCwd,
            sessionMode: sessionMode,
            providerConfigured: providerConfigured,
            providerProfilesJson: providerProfilesJson,
            providerRoutingJson: providerRoutingJson,
            defaultModelID: defaultModelID,
            visionDelegationEnabled: visionDelegationEnabled,
            mobileLinux: mobileLinux))
    }

    static func make(options: LaunchOptions = .init()) -> any ConversationSource {
        #if DEBUG
            if ProcessInfo.processInfo.environment["LINGXI_UI_TESTING"] == "1" {
                let source = MockConversationSource.uiTestFixture(
                    sessionMode: options.sessionMode,
                    cancelledRun: ProcessInfo.processInfo.environment["LINGXI_UI_TEST_CANCELLED_RUN"] == "1",
                    multiAgent: ProcessInfo.processInfo.environment["LINGXI_UI_TEST_MULTI_AGENT"] == "1",
                    holdTurn: ProcessInfo.processInfo.environment["LINGXI_UI_TEST_HOLD_TURN"] == "1",
                    askQuestion: ProcessInfo.processInfo.environment["LINGXI_UI_TEST_ASK_QUESTION"] == "1"
                )
                source.model.providerConfigured =
                    ProcessInfo.processInfo.environment["LINGXI_UI_TEST_PROVIDER_UNCONFIGURED"] != "1"
                return source
            }
        #endif
        #if canImport(harness_runtimeFFI)
            let env = ProcessInfo.processInfo.environment
            let isPreview = env["XCODE_RUNNING_FOR_PREVIEWS"] == "1"
            if !isPreview {
                let root = appSandboxRoot()
                // The shared engine restores its last confirmed model when the
                // provider is still available. These values only seed its fallback:
                // configured provider first, then the legacy Keychain model for
                // installs that have not migrated their provider configuration.
                let storedModel = options.defaultModelID ?? Keychain.get(.model) ?? ""
                let config = EngineConfig.fromEnvironment(
                    appSandboxRoot: root,
                    model: storedModel,
                    projectCwd: options.projectCwd,
                    sessionMode: options.sessionMode,
                    providerProfilesJson: options.providerProfilesJson,
                    providerRoutingJson: options.providerRoutingJson,
                    visionDelegationEnabled: options.visionDelegationEnabled,
                    mobileLinux: options.mobileLinux)
                let source = EngineConversationSource(config: config)
                source.model.providerConfigured = options.providerConfigured
                return source
            }
        #endif
        let source = MockConversationSource(sessionMode: options.sessionMode)
        source.model.providerConfigured = options.providerConfigured
        return source
    }

    /// The app's writable container root the engine roots its filesystem +
    /// `~/.claude`-equivalent under. Uses Application Support (created on demand).
    ///
    /// `nonisolated` because it reads `FileManager` and nothing else: the
    /// enclosing enum is `@MainActor` for the source-construction members, and
    /// inheriting that here would force every caller onto the main actor for a
    /// pure path computation. The mobile-linux FFI bridges — Settings, the
    /// terminal, cron — must reach this from synchronous nonisolated code,
    /// because it is the single authority for the root the ios-ish runtime
    /// validates mounts against, and a second copy for their benefit
    /// is exactly the divergence that left the wrong directory guarded.
    nonisolated static func appSandboxRoot() -> String {
        let fm = FileManager.default
        let base = (try? fm.url(for: .applicationSupportDirectory,
                                in: .userDomainMask,
                                appropriateFor: nil,
                                create: true))
            ?? fm.temporaryDirectory
        let root = base.appendingPathComponent("LingxiCode", isDirectory: true)
        try? fm.createDirectory(at: root, withIntermediateDirectories: true)
        return root.path
    }
}
