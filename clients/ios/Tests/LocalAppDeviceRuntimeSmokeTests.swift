import Foundation
import MobileLinuxNativeSupport
import XCTest

@testable import LingxiCode

/// Opt in on a freshly launched physical-device test host with
/// LINGXI_RUN_DEVICE_RUNTIME_SMOKE=1 in the test target's EnvironmentVariables.
/// Run only this class with test timeouts enabled and a 600-second allowance.
/// iSH kernel teardown requires process exit: the scratch directory is
/// intentionally retained until then, never removed while mounted.
final class LocalAppDeviceRuntimeSmokeTests: XCTestCase {
    func testBundledToolchainAndOfflineViteBuild() async throws {
        #if targetEnvironment(simulator)
            throw XCTSkip("Requires the physical-device Linux runtime")
        #else
            guard ProcessInfo.processInfo.environment["LINGXI_RUN_DEVICE_RUNTIME_SMOKE"] == "1" else {
                throw XCTSkip("Set LINGXI_RUN_DEVICE_RUNTIME_SMOKE=1 to opt in")
            }
            executionTimeAllowance = 600
            let report = try await Task.detached(priority: .userInitiated) {
                try Self.runSmoke()
            }.value
            let attachment = XCTAttachment(string: report)
            attachment.name = "Local App isolated device runtime smoke"
            attachment.lifetime = .keepAlways
            add(attachment)
        #endif
    }

    private struct SmokeFailure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    private static func runSmoke() throws -> String {
        guard !Thread.isMainThread else {
            throw SmokeFailure(message: "Synchronous native execution must not run on the main thread")
        }
        guard mlr_ish_is_available() else {
            throw SmokeFailure(message: "Physical-device native runtime bridge unavailable")
        }
        guard let seedPath = LocalAppsRuntimeDistribution.runtimeRoot else {
            throw SmokeFailure(message: "Bundled Local App dependency seed is missing")
        }
        let manager = FileManager.default
        let identifier = "device-runtime-smoke-\(UUID().uuidString.lowercased())"
        let scratch = manager.temporaryDirectory.appendingPathComponent(identifier, isDirectory: true)
        let workspace = scratch.appendingPathComponent("workspace", isDirectory: true)
        let seed = URL(fileURLWithPath: seedPath, isDirectory: true)
        try manager.createDirectory(at: scratch, withIntermediateDirectories: true)
        try manager.copyItem(at: seed.appendingPathComponent("template"), to: workspace)
        // The distribution is read-only; only the isolated build root needs a
        // writable directory entry for dist. Sources and seed remain unchanged.
        try manager.setAttributes([.posixPermissions: 0o755], ofItemAtPath: workspace.path)
        let modules = workspace.appendingPathComponent("node_modules", isDirectory: true)
        guard !manager.fileExists(atPath: modules.path) else {
            throw SmokeFailure(message: "Bundled template unexpectedly contains node_modules")
        }
        try manager.copyItem(at: seed.appendingPathComponent("node_modules"), to: modules)
        // Vite creates its disposable config cache beside the copied packages.
        try manager.setAttributes([.posixPermissions: 0o755], ofItemAtPath: modules.path)
        let script = #"""
        const fs = require('node:fs');
        const crypto = require('node:crypto');
        const { Worker } = require('node:worker_threads');
        const expectedVersion = process.argv[2];
        if (!expectedVersion || process.version !== expectedVersion) throw new Error(`unexpected Node ${process.version}; expected ${expectedVersion}`);
        fs.writeFileSync('smoke-file.txt', 'local-app-smoke');
        if (fs.readFileSync('smoke-file.txt', 'utf8') !== 'local-app-smoke') throw new Error('fs roundtrip failed');
        if (crypto.createHash('sha256').update('abc').digest('hex') !== 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad') throw new Error('crypto failed');
        const worker = new Worker("require('node:worker_threads').parentPort.postMessage(42)", { eval: true });
        let received = false;
        worker.on('message', value => { if (value !== 42) throw new Error('worker result'); received = true; });
        worker.on('error', error => { throw error; });
        worker.on('exit', code => { if (code !== 0 || !received) throw new Error('worker failed'); console.log(`NODE_FS_CRYPTO_WORKER_OK ${process.version}`); });
        """#
        try script.write(to: workspace.appendingPathComponent("runtime-smoke.cjs"), atomically: true, encoding: .utf8)
        let manifest = LXISHRuntimeBundleMetadata.current()
        let managedRoot = scratch.appendingPathComponent("managed", isDirectory: true).path
        let config: [String: Any] = [
            "managed_root": managedRoot,
            "workspace_host_path": workspace.path,
            "stable_workspace_id": identifier,
            "abi": "arm64",
            "rootfs_version": manifest.rootfsVersion,
            "archive_sha256": manifest.archiveSha256 as Any? ?? NSNull(),
            "authorization_file": LXISHRuntimeBundleResources.authorizationManifestURL()?.path as Any? ?? NSNull(),
            "rootfs_archive_path": LXISHRuntimeResourceBootstrap.rootfsArchiveURL(managedRoot: managedRoot)?.path as Any? ?? NSNull(),
            "default_mount_path": LXISHRuntimeResourceBootstrap.defaultMountURL(managedRoot: managedRoot)?.path as Any? ?? NSNull(),
            "rootfs_patch_path": LXISHRuntimeResourceBootstrap.rootfsPatchURL(managedRoot: managedRoot)?.path as Any? ?? NSNull(),
        ]
        let guestWorkspace = LingxiCode.LXISHGuestPaths.workspace(identifier)
        let command = #"""
        set -eux
        test "$(/usr/bin/node --version)" = "v26.9.0"
        test "$(/usr/bin/npm --version)" = "12.0.2"
        test "$(/usr/bin/pnpm --version)" = "12.5.1"
        test "$(/opt/lingxi/toolchains/legacy/bin/node --version)" = "v24.18.1"
        test "$(cd / && /opt/lingxi/toolchains/legacy/bin/pnpm --version)" = "11.22.0"
        /usr/bin/node runtime-smoke.cjs v26.9.0
        /opt/lingxi/toolchains/legacy/bin/node runtime-smoke.cjs v24.18.1
        /usr/bin/node -e "if(require('./node_modules/vite/package.json').version!=='8.3.0')throw Error('Vite version')"
        /usr/bin/node node_modules/vite/bin/vite.js build --config vite.config.mjs
        test -s dist/index.html
        echo OFFLINE_VITE_BUILD_OK
        """#
        let request: [String: Any] = [
            "command": "/bin/sh", "args": ["-c", command], "cwd": guestWorkspace,
            "env": ["PATH": "/usr/local/bin:/usr/bin:/bin", "NODE_OPTIONS": "--jitless", "CI": "1"],
            "stdin": NSNull(), "timeout_ms": 300_000, "network": "disabled",
            "resource_limits": NSNull(),
            "mounts": [["host_path": workspace.path, "guest_path": guestWorkspace,
                        "read_only": false, "purpose": "workspace"]],
            "include_default_mounts": false,
        ]
        let configJSON = String(decoding: try JSONSerialization.data(withJSONObject: config, options: [.sortedKeys]), as: UTF8.self)
        let requestJSON = String(decoding: try JSONSerialization.data(withJSONObject: request, options: [.sortedKeys]), as: UTF8.self)
        let response: String = try configJSON.withCString { configPointer in
            try requestJSON.withCString { requestPointer in
                guard let pointer = mlr_ish_run_sync_json(configPointer, requestPointer) else {
                    throw SmokeFailure(message: "Native smoke returned a null response; scratch: \(scratch.path)")
                }
                defer { mlr_ish_free_string(pointer) }
                return String(cString: pointer)
            }
        }
        try response.write(to: scratch.appendingPathComponent("response.json"), atomically: true, encoding: .utf8)
        NSLog("LOCAL_APP_SMOKE_RESPONSE %@", response)
        guard let envelope = try JSONSerialization.jsonObject(with: Data(response.utf8)) as? [String: Any],
              envelope["ok"] as? Bool == true,
              let result = envelope["result"] as? [String: Any],
              result["exit_code"] as? Int == 0,
              result["timed_out"] as? Bool == false,
              result["cancelled"] as? Bool == false,
              result["network_policy_enforced"] as? Bool == true,
              let output = result["stdout"] as? String,
              output.contains("NODE_FS_CRYPTO_WORKER_OK v26.9.0"),
              output.contains("NODE_FS_CRYPTO_WORKER_OK v24.18.1"),
              output.contains("OFFLINE_VITE_BUILD_OK")
        else {
            throw SmokeFailure(message: "Device runtime smoke failed. Scratch retained at \(scratch.path)\n\(response)")
        }
        return "Scratch retained until test host exits: \(scratch.path)\n\(response)"
    }
}
