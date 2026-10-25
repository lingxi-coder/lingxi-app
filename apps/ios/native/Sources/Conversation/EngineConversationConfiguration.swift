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
/// Runtime config for the in-process engine. Persisted credentials stay in the
/// engine's secure store; only an explicit development override is a scalar.
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
    var additionalWorkspaceDirectories: [String] = []
    /// Exact guest paths authorized by the native Project store for Mobile
    /// Linux bind mounts. This is separate from the file-tool trust catalog in
    /// `additionalWorkspaceDirectories`.
    var projectMountGrants: [String] = []

    /// Persisted API keys are resolved per request from native secure storage.
    /// An explicit environment override remains available for development/CI.
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
        let key = nonEmpty(env["ANTHROPIC_API_KEY"]) ?? ""
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

/// Retire the old app-specific slot before exposing any newly built engine.
/// Serializing bootstraps prevents a concurrent source/Cron launch from
/// reinserting a stale copy after the canonical key has been replaced/deleted.
actor LegacyAnthropicCredentialMigration {
    static let shared = LegacyAnthropicCredentialMigration()
    private var inFlight: Task<Void, Error>?

    func migrate(
        readLegacy: @escaping @Sendable () async throws -> String?,
        canonicalExists: @escaping @Sendable () async throws -> Bool,
        storeCanonical: @escaping @Sendable (String) async throws -> Void,
        retireLegacy: @escaping @Sendable () async throws -> Void
    ) async throws {
        if let inFlight { return try await inFlight.value }
        let operation = Task {
            guard let secret = try await readLegacy() else { return }
            if !(try await canonicalExists()) {
                try await storeCanonical(secret)
                guard try await canonicalExists() else {
                    throw NSError(domain: "ProviderCredentialMigration", code: 1, userInfo: [
                        NSLocalizedDescriptionKey: String(localized: "settings_provider_secure_storage_not_confirmed")
                    ])
                }
            }
            try await retireLegacy()
        }
        inFlight = operation
        defer { inFlight = nil }
        try await operation.value
    }
}

func migrateLegacyAnthropicCredential(on handle: MobileEngineHandle) async throws {
    try Task.checkCancellation()
    try await LegacyAnthropicCredentialMigration.shared.migrate(
        readLegacy: { Keychain.get(.apiKey) },
        canonicalExists: {
            try await SecureStorageImpl().retrieve(service: "lingxi", account: "anthropic-api-key") != nil
        },
        storeCanonical: { secret in
            try await handle.submit(command: .setProviderCredential(
                operationId: 0, providerId: "anthropic", credential: .init(value: secret)))
        },
        retireLegacy: {
            guard Keychain.clear(.apiKey) else {
                throw NSError(domain: "ProviderCredentialMigration", code: 2, userInfo: [
                    NSLocalizedDescriptionKey: String(localized: "settings_provider_legacy_keychain_delete_failed")
                ])
            }
        }
    )
    try Task.checkCancellation()
}
#endif
