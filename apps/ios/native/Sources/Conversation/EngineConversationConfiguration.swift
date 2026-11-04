import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
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
/// Runtime config for the in-process engine. Persisted credentials stay in the
/// engine's account authority.
struct EngineConfig {
    var apiBase: String
    var model: String
    var appSandboxRoot: String
    var projectCwd: String?
    var sessionMode: SessionMode
    var providerProfilesJson: String?
    var providerRoutingJson: String?
    var visionDelegationEnabled: Bool
    var mobileLinux: TerminalRuntimeConfig?
    var additionalWorkspaceDirectories: [String] = []
    /// Exact guest paths authorized by the native Project store for Mobile
    /// Linux bind mounts. This is separate from the file-tool trust catalog in
    /// `additionalWorkspaceDirectories`.
    var projectMountGrants: [String] = []
    var initialSessionId: String? = nil
    var initialSessionTranscriptPath: String? = nil

    /// Persisted API keys are resolved per request from native secure storage.
    static func fromEnvironment(appSandboxRoot: String,
                                model: String,
                                projectCwd: String? = nil,
                                sessionMode: SessionMode = .code,
                                providerProfilesJson: String? = nil,
                                providerRoutingJson: String? = nil,
                                visionDelegationEnabled: Bool = true,
                                mobileLinux: TerminalRuntimeConfig? = nil,
                                projectMountGrants: [String] = [],
                                environment env: [String: String] = ProcessInfo.processInfo.environment) -> EngineConfig {
        let base = nonEmpty(env["ANTHROPIC_BASE_URL"]) ?? "https://api.anthropic.com"
        return EngineConfig(
            apiBase: base,
            model: nonEmpty(env["LINGXI_MODEL"]) ?? model,
            appSandboxRoot: appSandboxRoot,
            projectCwd: projectCwd,
            sessionMode: sessionMode,
            providerProfilesJson: providerProfilesJson,
            providerRoutingJson: providerRoutingJson,
            visionDelegationEnabled: visionDelegationEnabled,
            mobileLinux: mobileLinux,
            projectMountGrants: projectMountGrants
        )
    }

    /// `s` when it is non-nil and non-empty, else `nil` — so an unset OR blank
    /// env var does not create an explicit override.
    private static func nonEmpty(_ s: String?) -> String? {
        guard let s, !s.isEmpty else { return nil }
        return s
    }
}

#endif
