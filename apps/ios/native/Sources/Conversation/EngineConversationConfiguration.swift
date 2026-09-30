import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)
/// Capture only stable native host facts once per engine construction.
/// Viewport, safe-area, theme, and model/provider choices are intentionally
/// excluded so the model-facing runtime context remains cacheable.
@MainActor
func makeIosHostEnvironment(
    launchMode: IosLaunchModeFfi
) -> IosHostEnvironmentFfi {
    let deviceClass: IosDeviceClassFfi = switch UIDevice.current.userInterfaceIdiom {
    case .phone: .phone
    case .pad: .tablet
    default: .unknown
    }
    #if targetEnvironment(simulator)
        let executionTarget = IosExecutionTargetFfi.simulator
    #else
        let executionTarget = IosExecutionTargetFfi.physicalDevice
    #endif
    return IosHostEnvironmentFfi(
        osVersion: UIDevice.current.systemVersion,
        deviceClass: deviceClass,
        executionTarget: executionTarget,
        launchMode: launchMode
    )
}
#endif

#if canImport(harness_runtimeFFI)
/// Runtime config for the in-process engine. The API key comes from the
/// environment / an app setting — never hardcoded.
struct EngineConfig {
    var apiBase: String
    var apiKey: String
    var model: String
    var appSandboxRoot: String
    var projectCwd: String?
    var sessionMode: SessionMode
    var providerProfilesJson: String?
    var providerRoutingJson: String?
    var visionDelegationEnabled: Bool
    var mobileLinux: TerminalRuntimeConfig?

    /// Resolve the engine credentials. The API key (and optional base URL)
    /// come from the iOS Keychain FIRST (SHIP-BLOCKER #1 — a shipped app has no
    /// process env), with an environment override for development/CI so a
    /// `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` in the env still wins for a
    /// dev run. An empty key is valid (turns 401 at run time, slash commands
    /// still work) and keeps the mock fallback in `make()`.
    static func fromEnvironment(appSandboxRoot: String,
                                model: String,
                                projectCwd: String? = nil,
                                sessionMode: SessionMode = .code,
                                providerProfilesJson: String? = nil,
                                providerRoutingJson: String? = nil,
                                visionDelegationEnabled: Bool = true,
                                mobileLinux: TerminalRuntimeConfig? = nil) -> EngineConfig {
        let env = ProcessInfo.processInfo.environment
        // Key: env override (dev) > Keychain (shipped) > empty.
        let key = nonEmpty(env["ANTHROPIC_API_KEY"])
            ?? Keychain.get(.apiKey)
            ?? ""
        // Base URL: env override (dev) > Keychain (shipped) > Anthropic default.
        let base = nonEmpty(env["ANTHROPIC_BASE_URL"])
            ?? Keychain.get(.apiBase)
            ?? "https://api.anthropic.com"
        // Model: env override (dev) > caller-supplied (Keychain) > "" (engine
        // default). SHIP-BLOCKER #2: an EMPTY result is the intended "let the
        // engine pick `MobileConfig.default_model`" signal — `build_ios_engine`
        // only overrides `default_model` when the passed id is non-empty.
        return EngineConfig(
            apiBase: base,
            apiKey: key,
            model: nonEmpty(env["LINGXI_MODEL"]) ?? model,
            appSandboxRoot: appSandboxRoot,
            projectCwd: projectCwd,
            sessionMode: sessionMode,
            providerProfilesJson: providerProfilesJson,
            providerRoutingJson: providerRoutingJson,
            visionDelegationEnabled: visionDelegationEnabled,
            mobileLinux: mobileLinux
        )
    }

    /// `s` when it is non-nil and non-empty, else `nil` — so an unset OR blank
    /// env var falls through to the Keychain instead of masking it with "".
    private static func nonEmpty(_ s: String?) -> String? {
        guard let s, !s.isEmpty else { return nil }
        return s
    }
}
#endif
