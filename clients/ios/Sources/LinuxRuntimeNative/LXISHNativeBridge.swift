//
//  LXISHNativeBridge.swift
//  LingxiCode
//
//  Swift implementation for the Rust-facing C ABI declared in
//  `LXISHNativeBridge.h`.
//

import Foundation
import Network
import ObjectiveC.runtime

struct LXISHMountSpec: Codable, Hashable {
    var hostPath: String
    var guestPath: String
    var readOnly: Bool
    var purpose: String

    enum CodingKeys: String, CodingKey {
        case hostPath = "host_path"
        case guestPath = "guest_path"
        case readOnly = "read_only"
        case purpose
    }
}

struct LXISHRunRequest: Codable {
    var command: String
    var args: [String]
    var cwd: String?
    var env: [String: String]
    var stdin: String?
    var timeoutMs: UInt64?
    var network: String
    var mounts: [LXISHMountSpec]?

    enum CodingKeys: String, CodingKey {
        case command, args, cwd, env, stdin, network, mounts
        case timeoutMs = "timeout_ms"
    }
}

struct LXISHPtyOpenRequest: Codable {
    var command: String
    var args: [String]
    var cwd: String?
    var env: [String: String]
    var cols: UInt16
    var rows: UInt16
    var mounts: [LXISHMountSpec]?
}

struct LXISHPtyWriteRequest: Codable {
    var sessionId: String
    var dataBase64: String

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case dataBase64 = "data_base64"
    }
}

struct LXISHPtyResizeRequest: Codable {
    var sessionId: String
    var cols: UInt16
    var rows: UInt16

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case cols, rows
    }
}

struct LXISHPtyCloseRequest: Codable {
    var sessionId: String

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
    }
}

struct LXISHPollRequest: Codable {
    var afterSequence: UInt64?
    var limit: UInt32?

    enum CodingKeys: String, CodingKey {
        case afterSequence = "after_sequence"
        case limit
    }
}

struct LXISHBackgroundProcessRequest: Codable {
    var processId: String

    enum CodingKeys: String, CodingKey {
        case processId = "process_id"
    }
}

struct LXISHBackgroundPollRequest: Codable {
    var processId: String
    var afterSequence: UInt64?
    var limit: UInt32?

    enum CodingKeys: String, CodingKey {
        case processId = "process_id"
        case afterSequence = "after_sequence"
        case limit
    }
}

struct LXISHLoopbackProbeRequest: Codable {
    var port: UInt16
    var timeoutMs: UInt32

    enum CodingKeys: String, CodingKey {
        case port
        case timeoutMs = "timeout_ms"
    }
}

private struct LXISHErrorPayload: Codable {
    var code: String
    var message: String
}

private struct LXISHRunResultPayload: Codable {
    var stdout: String
    var stderr: String
    var exitCode: Int
    var timedOut: Bool
    var cancelled: Bool
    var durationSeconds: Double

    enum CodingKeys: String, CodingKey {
        case stdout, stderr
        case exitCode = "exit_code"
        case timedOut = "timed_out"
        case cancelled
        case durationSeconds = "duration_seconds"
    }
}

private struct LXISHPtyEventPayload: Codable {
    var sequence: UInt64
    var sessionId: String
    var kind: String
    var dataBase64: String?
    var detail: String?

    enum CodingKeys: String, CodingKey {
        case sequence
        case sessionId = "session_id"
        case kind
        case dataBase64 = "data_base64"
        case detail
    }
}

private struct LXISHBackgroundEventPayload: Codable {
    var sequence: UInt64
    var processId: String
    var kind: String
    var line: String?
    var dataBase64: String?
    var exitCode: Int?
    var cancelled: Bool?
    var detail: String?

    enum CodingKeys: String, CodingKey {
        case sequence, kind, line, cancelled, detail
        case processId = "process_id"
        case dataBase64 = "data_base64"
        case exitCode = "exit_code"
    }
}

private struct LXISHShellExecutionResultBox {
    var exitCode: Int
    var errorCode: Int
    var stdoutText: String
    var stderrText: String
    var durationSeconds: Double
}

struct LXISHGuestEnvironment {
    static func merged(
        requestEnvironment: [String: String],
        cwd: String?,
        stableWorkspaceId: String
    ) -> [String: String] {
        var environment = requestEnvironment
        let workspaceGuestPath = "/workspace/\(stableWorkspaceId)"
        let defaultHome = "/root"
        let defaultPath = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        let resolvedCwd = cwd?.isEmpty == false ? cwd! : workspaceGuestPath
        let defaults: [String: String] = [
            "HOME": defaultHome,
            "PWD": resolvedCwd,
            "PATH": defaultPath,
            "TMPDIR": "/tmp",
            "TMP": "/tmp",
            "TEMP": "/tmp",
            "XDG_CACHE_HOME": "\(defaultHome)/.cache",
            "NPM_CONFIG_CACHE": "\(defaultHome)/.npm",
            "npm_config_cache": "\(defaultHome)/.npm",
            "PIP_CACHE_DIR": "\(defaultHome)/.cache/pip",
            "SSL_CERT_FILE": "/etc/ssl/cert.pem",
            "SSL_CERT_DIR": "/etc/ssl/certs",
            "GIT_SSL_CAINFO": "/etc/ssl/cert.pem"
        ]
        for (key, value) in defaults where environment[key] == nil {
            environment[key] = value
        }
        environment["GIT_CONFIG_COUNT"] = "1"
        environment["GIT_CONFIG_KEY_0"] = "safe.directory"
        environment["GIT_CONFIG_VALUE_0"] = workspaceGuestPath
        return environment
    }
}

struct LXISHRuntimeMountPlanner {
    static func effectiveMounts(
        requestedMounts: [LXISHMountSpec],
        config: LXISHNativeConfig
    ) -> [LXISHMountSpec] {
        let workspaceGuestPath = "/workspace/\(config.stableWorkspaceId)"
        var mounts: [LXISHMountSpec] = [
            LXISHMountSpec(
                hostPath: config.persistentHomeURL.path,
                guestPath: "/root",
                readOnly: false,
                purpose: "home"
            )
        ]
        if !config.workspaceHostPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            mounts.append(
                LXISHMountSpec(
                    hostPath: URL(fileURLWithPath: config.workspaceHostPath, isDirectory: true)
                        .standardizedFileURL
                        .path,
                    guestPath: workspaceGuestPath,
                    readOnly: false,
                    purpose: "workspace"
                )
            )
        }
        for mount in requestedMounts where mount.guestPath != "/root" && mount.guestPath != workspaceGuestPath {
            mounts.append(
                LXISHMountSpec(
                    hostPath: URL(fileURLWithPath: mount.hostPath, isDirectory: true)
                        .standardizedFileURL
                        .path,
                    guestPath: mount.guestPath,
                    readOnly: mount.readOnly,
                    purpose: mount.purpose
                )
            )
        }
        return mounts
    }
}

private enum LXISHBridgeError: LocalizedError {
    case invalidRequest(String)
    case unavailable(String)
    case io(String)

    var errorDescription: String? {
        switch self {
        case let .invalidRequest(message), let .unavailable(message), let .io(message):
            return message
        }
    }

    var code: String {
        switch self {
        case .invalidRequest: return "invalid_request"
        case .unavailable: return "unavailable"
        case .io: return "io"
        }
    }
}

private final class LXISHKernelRuntimeBridge {
    private var kernelObject: AnyObject?
    private var mountedGuestPaths: [String] = []
    private var interactiveShellOpen = false

    static func isDeviceBridgeAvailable() -> Bool {
        #if targetEnvironment(simulator)
        return false
        #else
        return NSClassFromString("ISHKernel") != nil
        #endif
    }

    static func availabilityReason() -> String {
        #if targetEnvironment(simulator)
        return "iSH bridge is disabled in the iOS Simulator"
        #else
        return isDeviceBridgeAvailable() ? "" : "OpenMinis ISHKernel is not linked into the app target"
        #endif
    }

    static func refreshDnsIfAvailable() {
        #if !targetEnvironment(simulator)
        do {
            try LXISHKernelRuntimeBridge().refreshDns()
        } catch {
            return
        }
        #endif
    }

    func boot(withRootPath rootPath: String) throws {
        let kernel = try resolveKernel()
        let selector = NSSelectorFromString("bootWithRootPath:")
        guard let method = class_getInstanceMethod(type(of: kernel), selector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing bootWithRootPath:")
        }
        typealias Fn = @convention(c) (AnyObject, Selector, NSString) -> Int32
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        let result = fn(kernel, selector, rootPath as NSString)
        guard result >= 0 else {
            throw LXISHBridgeError.unavailable("ISHKernel boot failed: \(result)")
        }
    }

    func configureMounts(_ mounts: [[String: Any]]) throws {
        let kernel = try resolveKernel()
        let unmountSelector = NSSelectorFromString("bindUnmountPath:")
        if let method = class_getInstanceMethod(type(of: kernel), unmountSelector) {
            typealias Fn = @convention(c) (AnyObject, Selector, NSString) -> Int32
            let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
            for guestPath in mountedGuestPaths.reversed() {
                _ = fn(kernel, unmountSelector, guestPath as NSString)
            }
        }
        mountedGuestPaths.removeAll()

        let mountSelector = NSSelectorFromString("bindMountPath:toHostPath:readOnly:")
        guard let method = class_getInstanceMethod(type(of: kernel), mountSelector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing bindMountPath:toHostPath:readOnly:")
        }
        typealias Fn = @convention(c) (AnyObject, Selector, NSString, NSString, ObjCBool) -> Int32
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        for mount in mounts {
            guard let guestPath = mount["guest_path"] as? String,
                  let hostPath = mount["host_path"] as? String
            else {
                throw LXISHBridgeError.invalidRequest("Mount entries require guest_path and host_path")
            }
            let readOnly = (mount["read_only"] as? Bool) ?? false
            let result = fn(kernel, mountSelector, guestPath as NSString, hostPath as NSString, ObjCBool(readOnly))
            guard result >= 0 else {
                throw LXISHBridgeError.unavailable("bind mount failed for \(guestPath) -> \(hostPath) (\(result))")
            }
            mountedGuestPaths.append(guestPath)
        }
    }

    func openInteractiveShell(
        withCommand command: [String],
        cols: UInt16,
        rows: UInt16,
        sink: @escaping (Data) -> Void
    ) throws {
        let kernel = try resolveKernel()
        let setSinkSelector = NSSelectorFromString("setOutputCallback:")
        if let method = class_getInstanceMethod(type(of: kernel), setSinkSelector) {
            typealias Fn = @convention(c) (AnyObject, Selector, AnyObject?) -> Void
            let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
            let block: @convention(block) (Data) -> Void = sink
            fn(kernel, setSinkSelector, unsafeBitCast(block, to: AnyObject.self))
        }
        try resizeColumns(cols, rows: rows)

        let executeSelector = NSSelectorFromString("executeCommand:")
        guard let method = class_getInstanceMethod(type(of: kernel), executeSelector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing executeCommand:")
        }
        typealias Fn = @convention(c) (AnyObject, Selector, NSArray) -> Int32
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        let result = fn(kernel, executeSelector, command as NSArray)
        guard result >= 0 else {
            throw LXISHBridgeError.unavailable("interactive shell launch failed: \(result)")
        }
        interactiveShellOpen = true
    }

    func writeInputData(_ data: Data) throws {
        let kernel = try resolveKernel()
        guard interactiveShellOpen else {
            throw LXISHBridgeError.invalidRequest("interactive shell is not open")
        }
        let selector = NSSelectorFromString("sendInput:")
        guard let method = class_getInstanceMethod(type(of: kernel), selector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing sendInput:")
        }
        typealias Fn = @convention(c) (AnyObject, Selector, NSData) -> Void
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        fn(kernel, selector, data as NSData)
    }

    func resizeColumns(_ cols: UInt16, rows: UInt16) throws {
        let kernel = try resolveKernel()
        let selector = NSSelectorFromString("setTerminalSize:rows:")
        guard let method = class_getInstanceMethod(type(of: kernel), selector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing setTerminalSize:rows:")
        }
        typealias Fn = @convention(c) (AnyObject, Selector, Int32, Int32) -> Void
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        fn(kernel, selector, Int32(cols), Int32(rows))
    }

    func refreshDns() throws {
        let kernel = try resolveKernel()
        let selector = NSSelectorFromString("refreshDns")
        guard let method = class_getInstanceMethod(type(of: kernel), selector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing refreshDns")
        }
        typealias Fn = @convention(c) (AnyObject, Selector) -> Void
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        fn(kernel, selector)
    }

    func closeInteractiveShell() throws {
        if interactiveShellOpen {
            try writeInputData(Data("exit\n".utf8))
        }
        interactiveShellOpen = false
    }

    private func resolveKernel() throws -> AnyObject {
        if let kernelObject {
            return kernelObject
        }
        guard let kernelClass = NSClassFromString("ISHKernel") else {
            throw LXISHBridgeError.unavailable(Self.availabilityReason())
        }
        let selector = NSSelectorFromString("shared")
        guard let method = class_getClassMethod(kernelClass, selector) else {
            throw LXISHBridgeError.unavailable("ISHKernel is missing shared")
        }
        typealias Fn = @convention(c) (AnyClass, Selector) -> AnyObject
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        let kernel = fn(kernelClass, selector)
        kernelObject = kernel
        return kernel
    }
}

private final class LXISHDNSRefreshMonitor {
    static let shared = LXISHDNSRefreshMonitor()

    private let lock = NSLock()
    private var monitor: NWPathMonitor?
    private var started = false

    func startIfNeeded() {
        #if targetEnvironment(simulator)
        return
        #else
        lock.lock()
        defer { lock.unlock() }
        guard !started else { return }
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            self?.handlePathUpdate(path)
        }
        monitor.start(queue: DispatchQueue(label: "com.lingxi.ish-native.dns-monitor"))
        self.monitor = monitor
        started = true
        #endif
    }

    /// NWPathMonitor only fires when the path actually changes, so refreshing on
    /// every satisfied update is not a busy loop — and it is the only policy
    /// that is correct here. Deduplicating on the interface-*type* set missed
    /// the most common real case: moving between two Wi-Fi networks keeps both
    /// the type and the `en0` interface identical while the resolvers behind
    /// them change completely, and NWPath exposes nothing that distinguishes
    /// them. Requiring a previous signature was wrong for the same reason in
    /// the other direction — it discarded the first update, which is the one
    /// that lands just after the runtime boots.
    private func handlePathUpdate(_ path: NWPath) {
        guard path.status == .satisfied else { return }
        LXISHKernelRuntimeBridge.refreshDnsIfAvailable()
    }
}

private final class LXISHShellExecutorRuntimeBridge {
    static func isDeviceBridgeAvailable() -> Bool {
        #if targetEnvironment(simulator)
        return false
        #else
        return NSClassFromString("ISHShellExecutor") != nil
        #endif
    }

    static func availabilityReason() -> String {
        #if targetEnvironment(simulator)
        return "iSH shell executor is disabled in the iOS Simulator"
        #else
        return isDeviceBridgeAvailable() ? "" : "OpenMinis ISHShellExecutor is not linked into the app target"
        #endif
    }

    func runExecutable(
        _ executable: String,
        arguments: [String],
        environment: [String: String],
        stdin: String?,
        cwd: String?,
        timeout: Double
    ) throws -> LXISHShellExecutionResultBox {
        guard let executorClass = NSClassFromString("ISHShellExecutor") else {
            throw LXISHBridgeError.unavailable(Self.availabilityReason())
        }
        let selector = NSSelectorFromString("executeExecutable:arguments:environment:stdinData:lineCallback:completion:")
        guard let method = class_getClassMethod(executorClass, selector) else {
            throw LXISHBridgeError.unavailable(
                "ISHShellExecutor is missing executeExecutable:arguments:environment:stdinData:lineCallback:completion:"
            )
        }

        let launch: (String, [String])
        if let cwd, !cwd.isEmpty {
            launch = (
                "/bin/sh",
                ["-c", "cd \"$1\" && shift && exec \"$@\"", "lingxi-run", cwd, executable] + arguments
            )
        } else {
            launch = (executable, arguments)
        }

        let semaphore = DispatchSemaphore(value: 0)
        let resultLock = NSLock()
        var completedResult: AnyObject?
        typealias CompletionBlock = @convention(block) (AnyObject) -> Void
        let completion: CompletionBlock = { result in
            resultLock.lock()
            completedResult = result
            resultLock.unlock()
            semaphore.signal()
        }
        typealias Fn = @convention(c) (
            AnyClass,
            Selector,
            NSString,
            NSArray,
            NSDictionary,
            NSData?,
            AnyObject?,
            AnyObject
        ) -> Int32
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        let stdinData = stdin.map { Data($0.utf8) } as NSData?
        let pid = fn(
            executorClass,
            selector,
            launch.0 as NSString,
            launch.1 as NSArray,
            environment as NSDictionary,
            stdinData,
            nil,
            completion as AnyObject
        )
        guard pid >= 0 else {
            throw LXISHBridgeError.unavailable("ISHShellExecutor failed to launch process: \(pid)")
        }

        let waitResult: DispatchTimeoutResult
        if timeout > 0 {
            waitResult = semaphore.wait(timeout: .now() + timeout)
        } else {
            semaphore.wait()
            waitResult = .success
        }
        if waitResult == .timedOut {
            killProcessGroup(pid, executorClass: executorClass)
            return LXISHShellExecutionResultBox(
                exitCode: -1,
                errorCode: -3,
                stdoutText: "",
                stderrText: "command timed out",
                durationSeconds: timeout
            )
        }

        resultLock.lock()
        let resultObject = completedResult
        resultLock.unlock()
        guard let resultObject else {
            throw LXISHBridgeError.unavailable("ISHShellExecutor completed without a result")
        }
        return LXISHShellExecutionResultBox(
            exitCode: intValue(from: resultObject, selector: "exitCode"),
            errorCode: intValue(from: resultObject, selector: "error"),
            stdoutText: stringValue(from: resultObject, selector: "output"),
            stderrText: stringValue(from: resultObject, selector: "errorOutput"),
            durationSeconds: doubleValue(from: resultObject, selector: "duration")
        )
    }

    func spawnExecutable(
        _ executable: String,
        arguments: [String],
        environment: [String: String],
        stdin: String?,
        cwd: String?,
        lineSink: @escaping (String, Bool) -> Void,
        completion: @escaping (LXISHShellExecutionResultBox) -> Void
    ) throws -> Int32 {
        guard let executorClass = NSClassFromString("ISHShellExecutor") else {
            throw LXISHBridgeError.unavailable(Self.availabilityReason())
        }
        let selector = NSSelectorFromString("executeExecutable:arguments:environment:stdinData:lineCallback:completion:")
        guard let method = class_getClassMethod(executorClass, selector) else {
            throw LXISHBridgeError.unavailable(
                "ISHShellExecutor is missing executeExecutable:arguments:environment:stdinData:lineCallback:completion:"
            )
        }

        let launch: (String, [String])
        if let cwd, !cwd.isEmpty {
            launch = (
                "/bin/sh",
                ["-c", "cd \"$1\" && shift && exec \"$@\"", "lingxi-background", cwd, executable] + arguments
            )
        } else {
            launch = (executable, arguments)
        }

        typealias LineBlock = @convention(block) (NSString, Bool) -> Void
        let lineBlock: LineBlock = { line, isStdErr in
            lineSink(line as String, isStdErr)
        }
        typealias CompletionBlock = @convention(block) (AnyObject) -> Void
        let completionBlock: CompletionBlock = { [weak self] result in
            guard let self else { return }
            completion(
                LXISHShellExecutionResultBox(
                    exitCode: self.intValue(from: result, selector: "exitCode"),
                    errorCode: self.intValue(from: result, selector: "error"),
                    stdoutText: self.stringValue(from: result, selector: "output"),
                    stderrText: self.stringValue(from: result, selector: "errorOutput"),
                    durationSeconds: self.doubleValue(from: result, selector: "duration")
                )
            )
        }
        typealias Fn = @convention(c) (
            AnyClass,
            Selector,
            NSString,
            NSArray,
            NSDictionary,
            NSData?,
            AnyObject?,
            AnyObject
        ) -> Int32
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        let pid = fn(
            executorClass,
            selector,
            launch.0 as NSString,
            launch.1 as NSArray,
            environment as NSDictionary,
            stdin.map { Data($0.utf8) } as NSData?,
            lineBlock as AnyObject,
            completionBlock as AnyObject
        )
        guard pid >= 0 else {
            throw LXISHBridgeError.unavailable("ISHShellExecutor failed to launch background process: \(pid)")
        }
        return pid
    }

    func killProcessGroup(_ pid: Int32) throws {
        guard pid > 1 else {
            throw LXISHBridgeError.invalidRequest("refusing to terminate iSH pid \(pid)")
        }
        guard let executorClass = NSClassFromString("ISHShellExecutor") else {
            throw LXISHBridgeError.unavailable(Self.availabilityReason())
        }
        let selector = NSSelectorFromString("killProcessGroup:")
        guard let method = class_getClassMethod(executorClass, selector) else {
            throw LXISHBridgeError.unavailable("ISHShellExecutor is missing killProcessGroup:")
        }
        typealias Fn = @convention(c) (AnyClass, Selector, Int32) -> Void
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        fn(executorClass, selector, pid)
    }

    private func killProcessGroup(_ pid: Int32, executorClass: AnyClass) {
        let selector = NSSelectorFromString("killProcessGroup:")
        guard let method = class_getClassMethod(executorClass, selector) else { return }
        typealias Fn = @convention(c) (AnyClass, Selector, Int32) -> Void
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        fn(executorClass, selector, pid)
    }

    private func intValue(from object: AnyObject, selector: String) -> Int {
        let sel = NSSelectorFromString(selector)
        guard let method = class_getInstanceMethod(type(of: object), sel) else { return 0 }
        typealias Fn = @convention(c) (AnyObject, Selector) -> Int32
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        return Int(fn(object, sel))
    }

    private func doubleValue(from object: AnyObject, selector: String) -> Double {
        let sel = NSSelectorFromString(selector)
        guard let method = class_getInstanceMethod(type(of: object), sel) else { return 0 }
        typealias Fn = @convention(c) (AnyObject, Selector) -> Double
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        return fn(object, sel)
    }

    private func stringValue(from object: AnyObject, selector: String) -> String {
        let sel = NSSelectorFromString(selector)
        guard let method = class_getInstanceMethod(type(of: object), sel) else { return "" }
        typealias Fn = @convention(c) (AnyObject, Selector) -> AnyObject?
        let fn = unsafeBitCast(method_getImplementation(method), to: Fn.self)
        return (fn(object, sel) as? String) ?? ""
    }
}

private final class LXISHNativeCoordinator {
    static let shared = LXISHNativeCoordinator()

    private final class BackgroundProcessState {
        let processId: String
        let guestPid: Int32
        var killRequested = false
        var terminal = false

        init(processId: String, guestPid: Int32) {
            self.processId = processId
            self.guestPid = guestPid
        }
    }

    private struct RuntimeState {
        var config: LXISHNativeConfig
        var kernel = LXISHKernelRuntimeBridge()
        var executor = LXISHShellExecutorRuntimeBridge()
        var mounts: [LXISHMountSpec] = []
        var kernelBooted = false
        var ptySessionId: String?
        var nextSequence: UInt64 = 0
        var events: [LXISHPtyEventPayload] = []
        var backgroundProcesses: [String: BackgroundProcessState] = [:]
        var backgroundEvents: [LXISHBackgroundEventPayload] = []
    }

    private let queue = DispatchQueue(label: "com.lingxi.ish-native.bridge")
    private let rootfsManager = LXISHNativeRootfsManager()
    private var runtimes: [String: RuntimeState] = [:]

    func availability() -> String {
        encodeEnvelope(
            ok: LXISHKernelRuntimeBridge.isDeviceBridgeAvailable() && LXISHShellExecutorRuntimeBridge.isDeviceBridgeAvailable(),
            payload: [
                "available": LXISHKernelRuntimeBridge.isDeviceBridgeAvailable() && LXISHShellExecutorRuntimeBridge.isDeviceBridgeAvailable(),
                "kernel_available": LXISHKernelRuntimeBridge.isDeviceBridgeAvailable(),
                "shell_executor_available": LXISHShellExecutorRuntimeBridge.isDeviceBridgeAvailable(),
                "backend": "ios-ish",
                "kernel_reason": LXISHKernelRuntimeBridge.availabilityReason(),
                "shell_reason": LXISHShellExecutorRuntimeBridge.availabilityReason()
            ]
        )
    }

    func installRootfs(config: LXISHNativeConfig) -> String {
        execute(config: config) { runtime in
            let status = try self.rootfsManager.installIfNeeded(for: config)
            runtime.mounts = self.rootfsManager.cachedMounts(for: config)
            return ["status": status]
        }
    }

    func repairRootfs(config: LXISHNativeConfig) -> String {
        execute(config: config) { runtime in
            guard !runtime.kernelBooted else {
                throw LXISHBridgeError.io("restart the app before repairing a booted iSH rootfs")
            }
            let status = try self.rootfsManager.repair(for: config)
            runtime.mounts = self.rootfsManager.cachedMounts(for: config)
            return ["status": status]
        }
    }

    func resetRootfs(config: LXISHNativeConfig) -> String {
        execute(config: config) { runtime in
            guard !runtime.kernelBooted else {
                throw LXISHBridgeError.io("restart the app before resetting a booted iSH rootfs")
            }
            let status = try self.rootfsManager.reset(for: config)
            runtime.mounts = []
            runtime.ptySessionId = nil
            runtime.events.removeAll()
            return ["status": status]
        }
    }

    func boot(config: LXISHNativeConfig) -> String {
        execute(config: config) { runtime in
            _ = try self.rootfsManager.installIfNeeded(for: config)
            runtime.mounts = self.rootfsManager.cachedMounts(for: config)
            guard LXISHKernelRuntimeBridge.isDeviceBridgeAvailable() else {
                throw LXISHBridgeError.unavailable(LXISHKernelRuntimeBridge.availabilityReason())
            }
            do {
                try runtime.kernel.boot(withRootPath: config.rootfsURL.path)
                runtime.kernelBooted = true
                try self.applyMountsIfNeeded(to: &runtime)
            } catch {
                throw LXISHBridgeError.unavailable(error.localizedDescription)
            }
            let status = self.rootfsManager.status(for: config)
            return ["status": status]
        }
    }

    func configureMounts(config: LXISHNativeConfig, mounts: [LXISHMountSpec]) -> String {
        execute(config: config) { runtime in
            runtime.mounts = mounts
            try self.rootfsManager.cacheMounts(mounts, for: config)
            try self.ensureHostMountsExist(mounts)
            if LXISHKernelRuntimeBridge.isDeviceBridgeAvailable() {
                try runtime.kernel.configureMounts(mounts.map(self.dictionary(from:)))
            }
            return ["status": self.rootfsManager.status(for: config)]
        }
    }

    func runSync(config: LXISHNativeConfig, request: LXISHRunRequest) -> String {
        execute(config: config) { runtime in
            guard request.network == "allowed" else {
                throw LXISHBridgeError.unavailable(
                    "iSH cannot enforce the requested network isolation policy"
                )
            }
            let environment = self.preparedEnvironment(from: request.env, cwd: request.cwd, config: config)
            _ = try self.rootfsManager.installIfNeeded(for: config)
            runtime.mounts = request.mounts ?? runtime.mounts
            try self.rootfsManager.cacheMounts(runtime.mounts, for: config)
            guard LXISHKernelRuntimeBridge.isDeviceBridgeAvailable(), LXISHShellExecutorRuntimeBridge.isDeviceBridgeAvailable() else {
                throw LXISHBridgeError.unavailable(LXISHShellExecutorRuntimeBridge.availabilityReason())
            }
            try runtime.kernel.boot(withRootPath: config.rootfsURL.path)
            runtime.kernelBooted = true
            try self.applyMountsIfNeeded(to: &runtime)
            try self.validateEnvironment(environment)
            let result = try runtime.executor.runExecutable(
                request.command,
                arguments: request.args,
                environment: environment,
                stdin: request.stdin,
                cwd: request.cwd,
                timeout: self.timeoutSeconds(from: request)
            )
            return [
                "result": LXISHRunResultPayload(
                    stdout: result.stdoutText,
                    stderr: result.stderrText,
                    exitCode: result.exitCode,
                    timedOut: result.errorCode == -3,
                    cancelled: result.errorCode == -4,
                    durationSeconds: result.durationSeconds
                )
            ]
        }
    }

    func spawnBackground(config: LXISHNativeConfig, request: LXISHRunRequest) -> String {
        execute(config: config) { runtime in
            guard request.network == "allowed" else {
                throw LXISHBridgeError.unavailable(
                    "iSH cannot enforce the requested network isolation policy"
                )
            }
            let environment = self.preparedEnvironment(from: request.env, cwd: request.cwd, config: config)
            _ = try self.rootfsManager.installIfNeeded(for: config)
            runtime.mounts = request.mounts ?? runtime.mounts
            try self.rootfsManager.cacheMounts(runtime.mounts, for: config)
            guard LXISHKernelRuntimeBridge.isDeviceBridgeAvailable(),
                  LXISHShellExecutorRuntimeBridge.isDeviceBridgeAvailable()
            else {
                throw LXISHBridgeError.unavailable(LXISHShellExecutorRuntimeBridge.availabilityReason())
            }
            try runtime.kernel.boot(withRootPath: config.rootfsURL.path)
            runtime.kernelBooted = true
            try self.applyMountsIfNeeded(to: &runtime)
            try self.validateEnvironment(environment)

            let processId = UUID().uuidString.lowercased()
            let runtimeKey = config.normalizedManagedRoot.path
            let pid = try runtime.executor.spawnExecutable(
                request.command,
                arguments: request.args,
                environment: environment,
                stdin: request.stdin,
                cwd: request.cwd,
                lineSink: { [weak self] line, isStdErr in
                    self?.recordBackgroundLine(
                        runtimeKey: runtimeKey,
                        processId: processId,
                        line: line,
                        isStdErr: isStdErr
                    )
                },
                completion: { [weak self] result in
                    self?.recordBackgroundCompletion(
                        runtimeKey: runtimeKey,
                        processId: processId,
                        result: result
                    )
                }
            )
            runtime.backgroundProcesses[processId] = BackgroundProcessState(
                processId: processId,
                guestPid: pid
            )
            return ["process_id": processId, "guest_pid": Int(pid)]
        }
    }

    func killBackground(config: LXISHNativeConfig, request: LXISHBackgroundProcessRequest) -> String {
        execute(config: config) { runtime in
            guard let process = runtime.backgroundProcesses[request.processId] else {
                throw LXISHBridgeError.invalidRequest("unknown background process")
            }
            if process.terminal {
                return ["process_id": request.processId, "already_stopped": true]
            }
            try runtime.executor.killProcessGroup(process.guestPid)
            process.killRequested = true
            return ["process_id": request.processId, "termination_requested": true]
        }
    }

    func pollBackground(config: LXISHNativeConfig, request: LXISHBackgroundPollRequest) -> String {
        queue.sync {
            let key = config.normalizedManagedRoot.path
            guard let runtime = runtimes[key] else {
                return encodeEnvelope(ok: true, payload: ["events": [LXISHBackgroundEventPayload]()])
            }
            guard runtime.backgroundProcesses[request.processId] != nil else {
                return encodeError(code: "invalid_request", message: "unknown background process")
            }
            let filtered = runtime.backgroundEvents.filter { event in
                guard event.processId == request.processId else { return false }
                guard let after = request.afterSequence else { return true }
                return event.sequence > after
            }
            let limit = Int(request.limit ?? UInt32.max)
            return encodeEnvelope(ok: true, payload: ["events": Array(filtered.prefix(limit))])
        }
    }

    func probeLoopback(config: LXISHNativeConfig, request: LXISHLoopbackProbeRequest) -> String {
        guard request.port > 0,
              let port = NWEndpoint.Port(rawValue: request.port)
        else {
            return encodeError(code: "invalid_request", message: "loopback port must be greater than zero")
        }
        let semaphore = DispatchSemaphore(value: 0)
        let resultLock = NSLock()
        var reachable = false
        var finished = false
        let connection = NWConnection(host: "127.0.0.1", port: port, using: .tcp)
        connection.stateUpdateHandler = { state in
            switch state {
            case .ready:
                resultLock.lock()
                if !finished {
                    reachable = true
                    finished = true
                    semaphore.signal()
                }
                resultLock.unlock()
            case .failed, .cancelled:
                resultLock.lock()
                if !finished {
                    finished = true
                    semaphore.signal()
                }
                resultLock.unlock()
            default:
                break
            }
        }
        connection.start(queue: DispatchQueue(label: "com.lingxi.ish-native.loopback-probe"))
        _ = semaphore.wait(timeout: .now() + .milliseconds(Int(request.timeoutMs)))
        connection.cancel()
        resultLock.lock()
        let result = reachable
        resultLock.unlock()
        return encodeEnvelope(ok: true, payload: ["reachable": result])
    }

    func openPty(config: LXISHNativeConfig, request: LXISHPtyOpenRequest) -> String {
        execute(config: config) { runtime in
            let environment = self.preparedEnvironment(from: request.env, cwd: request.cwd, config: config)
            _ = try self.rootfsManager.installIfNeeded(for: config)
            runtime.mounts = request.mounts ?? runtime.mounts
            try self.rootfsManager.cacheMounts(runtime.mounts, for: config)
            guard LXISHKernelRuntimeBridge.isDeviceBridgeAvailable() else {
                throw LXISHBridgeError.unavailable(LXISHKernelRuntimeBridge.availabilityReason())
            }
            try runtime.kernel.boot(withRootPath: config.rootfsURL.path)
            runtime.kernelBooted = true
            try self.applyMountsIfNeeded(to: &runtime)
            if runtime.ptySessionId != nil {
                throw LXISHBridgeError.unavailable("only one interactive PTY session is supported per managed root")
            }
            let sessionId = UUID().uuidString.lowercased()
            try self.validateEnvironment(environment)
            let ptyCommand = self.ptyCommand(
                from: LXISHPtyOpenRequest(
                    command: request.command,
                    args: request.args,
                    cwd: request.cwd,
                    env: environment,
                    cols: request.cols,
                    rows: request.rows,
                    mounts: request.mounts
                )
            )
            try runtime.kernel.openInteractiveShell(
                withCommand: ptyCommand,
                cols: request.cols,
                rows: request.rows
            ) { data in
                self.queue.async {
                    guard var current = self.runtimes[config.normalizedManagedRoot.path] else { return }
                    current.nextSequence += 1
                    current.events.append(
                        LXISHPtyEventPayload(
                            sequence: current.nextSequence,
                            sessionId: sessionId,
                            kind: "pty_output",
                            dataBase64: data.base64EncodedString(),
                            detail: nil
                        )
                    )
                    self.runtimes[config.normalizedManagedRoot.path] = current
                }
            }
            runtime.ptySessionId = sessionId
            return ["session_id": sessionId, "available": true]
        }
    }

    func writePty(config: LXISHNativeConfig, request: LXISHPtyWriteRequest) -> String {
        execute(config: config) { runtime in
            guard runtime.ptySessionId == request.sessionId else {
                throw LXISHBridgeError.invalidRequest("unknown PTY session")
            }
            guard let data = Data(base64Encoded: request.dataBase64) else {
                throw LXISHBridgeError.invalidRequest("data_base64 is not valid base64")
            }
            try runtime.kernel.writeInputData(data)
            return ["session_id": request.sessionId]
        }
    }

    func resizePty(config: LXISHNativeConfig, request: LXISHPtyResizeRequest) -> String {
        execute(config: config) { runtime in
            guard runtime.ptySessionId == request.sessionId else {
                throw LXISHBridgeError.invalidRequest("unknown PTY session")
            }
            try runtime.kernel.resizeColumns(request.cols, rows: request.rows)
            return ["session_id": request.sessionId]
        }
    }

    func closePty(config: LXISHNativeConfig, request: LXISHPtyCloseRequest) -> String {
        execute(config: config) { runtime in
            guard runtime.ptySessionId == request.sessionId else {
                throw LXISHBridgeError.invalidRequest("unknown PTY session")
            }
            try runtime.kernel.closeInteractiveShell()
            runtime.nextSequence += 1
            runtime.events.append(
                LXISHPtyEventPayload(
                    sequence: runtime.nextSequence,
                    sessionId: request.sessionId,
                    kind: "pty_closed",
                    dataBase64: nil,
                    detail: nil
                )
            )
            runtime.ptySessionId = nil
            return ["session_id": request.sessionId]
        }
    }

    func pollOutput(config: LXISHNativeConfig, request: LXISHPollRequest) -> String {
        queue.sync {
            let key = config.normalizedManagedRoot.path
            guard let runtime = runtimes[key] else {
                return encodeEnvelope(ok: true, payload: ["events": [LXISHPtyEventPayload]()])
            }
            let filtered = runtime.events.filter { event in
                guard let after = request.afterSequence else { return true }
                return event.sequence > after
            }
            let limit = Int(request.limit ?? UInt32.max)
            return encodeEnvelope(ok: true, payload: ["events": Array(filtered.prefix(limit))])
        }
    }

    private func recordBackgroundLine(
        runtimeKey: String,
        processId: String,
        line: String,
        isStdErr: Bool
    ) {
        queue.async {
            guard var runtime = self.runtimes[runtimeKey],
                  let process = runtime.backgroundProcesses[processId],
                  !process.terminal
            else { return }
            runtime.nextSequence += 1
            runtime.backgroundEvents.append(
                LXISHBackgroundEventPayload(
                    sequence: runtime.nextSequence,
                    processId: processId,
                    kind: isStdErr ? "stderr_chunk" : "stdout_line",
                    line: isStdErr ? nil : line,
                    dataBase64: isStdErr ? Data("\(line)\n".utf8).base64EncodedString() : nil,
                    exitCode: nil,
                    cancelled: nil,
                    detail: nil
                )
            )
            self.trimBackgroundEvents(&runtime)
            self.runtimes[runtimeKey] = runtime
        }
    }

    private func recordBackgroundCompletion(
        runtimeKey: String,
        processId: String,
        result: LXISHShellExecutionResultBox
    ) {
        queue.async {
            guard var runtime = self.runtimes[runtimeKey],
                  let process = runtime.backgroundProcesses[processId],
                  !process.terminal
            else { return }
            process.terminal = true
            let cancelled = process.killRequested || result.errorCode == -4
            let detail: String?
            switch result.errorCode {
            case -3: detail = "background process timed out"
            case -4: detail = "background process cancelled"
            case 0: detail = nil
            default: detail = "background process failed with executor error \(result.errorCode)"
            }
            runtime.nextSequence += 1
            runtime.backgroundEvents.append(
                LXISHBackgroundEventPayload(
                    sequence: runtime.nextSequence,
                    processId: processId,
                    kind: "process_exited",
                    line: nil,
                    dataBase64: nil,
                    exitCode: result.exitCode,
                    cancelled: cancelled,
                    detail: detail
                )
            )
            self.trimBackgroundEvents(&runtime)
            self.runtimes[runtimeKey] = runtime
        }
    }

    private func trimBackgroundEvents(_ runtime: inout RuntimeState) {
        let overflow = runtime.backgroundEvents.count - 4096
        if overflow > 0 {
            runtime.backgroundEvents.removeFirst(overflow)
        }
    }

    private func execute(config: LXISHNativeConfig, work: (inout RuntimeState) throws -> [String: Any]) -> String {
        queue.sync {
            var runtime = runtimes[config.normalizedManagedRoot.path] ?? RuntimeState(config: config)
            runtime.config = config
            do {
                LXISHDNSRefreshMonitor.shared.startIfNeeded()
                let payload = try work(&runtime)
                runtimes[config.normalizedManagedRoot.path] = runtime
                return encodeEnvelope(ok: true, payload: payload)
            } catch let error as LXISHBridgeError {
                runtimes[config.normalizedManagedRoot.path] = runtime
                return encodeError(code: error.code, message: error.localizedDescription)
            } catch {
                runtimes[config.normalizedManagedRoot.path] = runtime
                return encodeError(code: "io", message: error.localizedDescription)
            }
        }
    }

    private func applyMountsIfNeeded(to runtime: inout RuntimeState) throws {
        let mounts = LXISHRuntimeMountPlanner.effectiveMounts(
            requestedMounts: runtime.mounts,
            config: runtime.config
        )
        try ensureHostMountsExist(mounts)
        guard !mounts.isEmpty else { return }
        try runtime.kernel.configureMounts(mounts.map(dictionary(from:)))
    }

    private func ensureHostMountsExist(_ mounts: [LXISHMountSpec]) throws {
        for mount in mounts {
            try FileManager.default.createDirectory(
                at: URL(fileURLWithPath: mount.hostPath, isDirectory: true),
                withIntermediateDirectories: true
            )
        }
    }

    private func timeoutSeconds(from request: LXISHRunRequest) -> Double {
        guard let timeoutMs = request.timeoutMs else { return 0 }
        return Double(timeoutMs) / 1000
    }

    private func preparedEnvironment(
        from requestEnvironment: [String: String],
        cwd: String?,
        config: LXISHNativeConfig
    ) -> [String: String] {
        LXISHGuestEnvironment.merged(
            requestEnvironment: requestEnvironment,
            cwd: cwd,
            stableWorkspaceId: config.stableWorkspaceId
        )
    }

    private func ptyCommand(from request: LXISHPtyOpenRequest) -> [String] {
        guard request.cwd?.isEmpty == false || !request.env.isEmpty else {
            return [request.command] + request.args
        }
        let script = """
        if [ -n "$1" ]; then cd "$1" || exit $?; fi
        shift
        while [ "$1" != "--" ]; do export "$1" || exit $?; shift; done
        shift
        exec "$@"
        """
        let environment = request.env
            .sorted { $0.key < $1.key }
            .map { "\($0.key)=\($0.value)" }
        return ["/bin/sh", "-c", script, "lingxi-pty", request.cwd ?? ""]
            + environment + ["--", request.command] + request.args
    }

    private func validateEnvironment(_ environment: [String: String]) throws {
        for (key, value) in environment {
            guard !key.isEmpty,
                  !key.contains("="),
                  !key.utf8.contains(0),
                  !value.utf8.contains(0)
            else {
                throw LXISHBridgeError.invalidRequest("environment contains an invalid key or NUL byte")
            }
        }
    }

    private func dictionary(from mount: LXISHMountSpec) -> [String: Any] {
        [
            "host_path": mount.hostPath,
            "guest_path": mount.guestPath,
            "read_only": mount.readOnly,
            "purpose": mount.purpose
        ]
    }
}

private struct AnyEncodable: Encodable {
    private let encodeImpl: (Encoder) throws -> Void

    init<T: Encodable>(_ value: T) {
        encodeImpl = value.encode
    }

    func encode(to encoder: Encoder) throws {
        try encodeImpl(encoder)
    }
}

private struct Envelope: Encodable {
    var ok: Bool
    var payload: [String: AnyEncodable]

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: DynamicCodingKey.self)
        try container.encode(ok, forKey: DynamicCodingKey("ok"))
        for (key, value) in payload {
            try container.encode(value, forKey: DynamicCodingKey(key))
        }
    }
}

private struct DynamicCodingKey: CodingKey {
    var stringValue: String
    var intValue: Int?

    init(_ stringValue: String) {
        self.stringValue = stringValue
        self.intValue = nil
    }

    init?(stringValue: String) {
        self.init(stringValue)
    }

    init?(intValue: Int) {
        self.stringValue = "\(intValue)"
        self.intValue = intValue
    }
}

private func encodeEnvelope(ok: Bool, payload: [String: Any]) -> String {
    let converted = payload.reduce(into: [String: AnyEncodable]()) { result, item in
        switch item.value {
        case let value as AnyEncodable:
            result[item.key] = value
        case let value as String:
            result[item.key] = AnyEncodable(value)
        case let value as Bool:
            result[item.key] = AnyEncodable(value)
        case let value as Int:
            result[item.key] = AnyEncodable(value)
        case let value as UInt64:
            result[item.key] = AnyEncodable(value)
        case let value as UInt32:
            result[item.key] = AnyEncodable(value)
        case let value as [LXISHPtyEventPayload]:
            result[item.key] = AnyEncodable(value)
        case let value as [LXISHBackgroundEventPayload]:
            result[item.key] = AnyEncodable(value)
        case let value as LXISHRootfsStatus:
            result[item.key] = AnyEncodable(value)
        case let value as LXISHRunResultPayload:
            result[item.key] = AnyEncodable(value)
        default:
            break
        }
    }
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    let data = (try? encoder.encode(Envelope(ok: ok, payload: converted))) ?? Data("{\"ok\":false}".utf8)
    return String(decoding: data, as: UTF8.self)
}

private func encodeError(code: String, message: String) -> String {
    encodeEnvelope(ok: false, payload: ["error": AnyEncodable(LXISHErrorPayload(code: code, message: message))])
}

private func decodeConfig(_ pointer: UnsafePointer<CChar>?) throws -> LXISHNativeConfig {
    try decode(pointer, as: LXISHNativeConfig.self)
}

private func decode<T: Decodable>(_ pointer: UnsafePointer<CChar>?, as type: T.Type) throws -> T {
    guard let pointer else {
        throw LXISHBridgeError.invalidRequest("missing JSON payload")
    }
    let string = String(cString: pointer)
    guard let data = string.data(using: .utf8) else {
        throw LXISHBridgeError.invalidRequest("payload is not valid UTF-8")
    }
    do {
        return try JSONDecoder().decode(T.self, from: data)
    } catch {
        throw LXISHBridgeError.invalidRequest(error.localizedDescription)
    }
}

private func bridgeString(_ value: String) -> UnsafeMutablePointer<CChar>? {
    strdup(value)
}

private func bridgingResult(_ block: () throws -> String) -> UnsafeMutablePointer<CChar>? {
    do {
        return bridgeString(try block())
    } catch let error as LXISHBridgeError {
        return bridgeString(encodeError(code: error.code, message: error.localizedDescription))
    } catch {
        return bridgeString(encodeError(code: "io", message: error.localizedDescription))
    }
}

private func aggregatePtyReadJSON(from envelopeString: String) -> String {
    guard let data = envelopeString.data(using: .utf8),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    else {
        return envelopeString
    }
    guard json["ok"] as? Bool == true else {
        if let error = json["error"],
           let payload = try? JSONSerialization.data(withJSONObject: ["error": error], options: [.sortedKeys])
        {
            return String(decoding: payload, as: UTF8.self)
        }
        return envelopeString
    }
    let events = json["events"] as? [[String: Any]] ?? []
    var combined = Data()
    var lastSequence: UInt64 = 0
    var sessionId: String?
    var closed = false
    for event in events {
        if let sequence = event["sequence"] as? NSNumber {
            lastSequence = max(lastSequence, sequence.uint64Value)
        }
        if sessionId == nil {
            sessionId = event["session_id"] as? String
        }
        if let kind = event["kind"] as? String {
            if kind == "pty_output",
               let base64 = event["data_base64"] as? String,
               let chunk = Data(base64Encoded: base64) {
                combined.append(chunk)
            } else if kind == "pty_closed" {
                closed = true
            }
        }
    }
    let payload: [String: Any] = [
        "session_id": sessionId ?? "",
        "data_base64": combined.base64EncodedString(),
        "last_sequence": lastSequence,
        "closed": closed
    ]
    guard let payloadData = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) else {
        return envelopeString
    }
    return String(decoding: payloadData, as: UTF8.self)
}

@_cdecl("lx_ish_native_is_available")
func lx_ish_native_is_available() -> Bool {
    LXISHKernelRuntimeBridge.isDeviceBridgeAvailable() && LXISHShellExecutorRuntimeBridge.isDeviceBridgeAvailable()
}

@_cdecl("lx_ish_native_availability_json")
func lx_ish_native_availability_json() -> UnsafeMutablePointer<CChar>? {
    bridgeString(LXISHNativeCoordinator.shared.availability())
}

@_cdecl("lx_ish_native_install_rootfs_json")
func lx_ish_native_install_rootfs_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        LXISHNativeCoordinator.shared.installRootfs(config: try decodeConfig(configJSON))
    }
}

@_cdecl("lx_ish_native_repair_rootfs_json")
func lx_ish_native_repair_rootfs_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        LXISHNativeCoordinator.shared.repairRootfs(config: try decodeConfig(configJSON))
    }
}

@_cdecl("lx_ish_native_reset_rootfs_json")
func lx_ish_native_reset_rootfs_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        LXISHNativeCoordinator.shared.resetRootfs(config: try decodeConfig(configJSON))
    }
}

@_cdecl("lx_ish_native_boot_json")
func lx_ish_native_boot_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        LXISHNativeCoordinator.shared.boot(config: try decodeConfig(configJSON))
    }
}

private struct LXISHMountsEnvelope: Codable {
    var mounts: [LXISHMountSpec]
}

@_cdecl("lx_ish_native_configure_mounts_json")
func lx_ish_native_configure_mounts_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ mountsJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let mounts = try decode(mountsJSON, as: LXISHMountsEnvelope.self)
        return LXISHNativeCoordinator.shared.configureMounts(config: config, mounts: mounts.mounts)
    }
}

@_cdecl("lx_ish_native_run_sync_json")
func lx_ish_native_run_sync_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHRunRequest.self)
        return LXISHNativeCoordinator.shared.runSync(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_background_spawn_json")
func lx_ish_native_background_spawn_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHRunRequest.self)
        return LXISHNativeCoordinator.shared.spawnBackground(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_background_kill_json")
func lx_ish_native_background_kill_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHBackgroundProcessRequest.self)
        return LXISHNativeCoordinator.shared.killBackground(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_background_poll_json")
func lx_ish_native_background_poll_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHBackgroundPollRequest.self)
        return LXISHNativeCoordinator.shared.pollBackground(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_probe_loopback_json")
func lx_ish_native_probe_loopback_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHLoopbackProbeRequest.self)
        return LXISHNativeCoordinator.shared.probeLoopback(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_pty_open_json")
func lx_ish_native_pty_open_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHPtyOpenRequest.self)
        return LXISHNativeCoordinator.shared.openPty(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_pty_write_json")
func lx_ish_native_pty_write_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHPtyWriteRequest.self)
        return LXISHNativeCoordinator.shared.writePty(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_pty_resize_json")
func lx_ish_native_pty_resize_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHPtyResizeRequest.self)
        return LXISHNativeCoordinator.shared.resizePty(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_pty_close_json")
func lx_ish_native_pty_close_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHPtyCloseRequest.self)
        return LXISHNativeCoordinator.shared.closePty(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_poll_output_json")
func lx_ish_native_poll_output_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    bridgingResult {
        let config = try decodeConfig(configJSON)
        let request = try decode(requestJSON, as: LXISHPollRequest.self)
        return LXISHNativeCoordinator.shared.pollOutput(config: config, request: request)
    }
}

@_cdecl("lx_ish_native_free_string")
func lx_ish_native_free_string(_ value: UnsafeMutablePointer<CChar>?) {
    guard let value else { return }
    free(value)
}

@_cdecl("lingxi_ish_is_available")
func lingxi_ish_is_available() -> Bool {
    lx_ish_native_is_available()
}

@_cdecl("lingxi_ish_availability_json")
func lingxi_ish_availability_json() -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_availability_json()
}

@_cdecl("lingxi_ish_install_rootfs_json")
func lingxi_ish_install_rootfs_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_install_rootfs_json(configJSON)
}

@_cdecl("lingxi_ish_repair_rootfs_json")
func lingxi_ish_repair_rootfs_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_repair_rootfs_json(configJSON)
}

@_cdecl("lingxi_ish_reset_rootfs_json")
func lingxi_ish_reset_rootfs_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_reset_rootfs_json(configJSON)
}

@_cdecl("lingxi_ish_boot_json")
func lingxi_ish_boot_json(_ configJSON: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_boot_json(configJSON)
}

@_cdecl("lingxi_ish_configure_mounts_json")
func lingxi_ish_configure_mounts_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ mountsJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_configure_mounts_json(configJSON, mountsJSON)
}

@_cdecl("lingxi_ish_run_json")
func lingxi_ish_run_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_run_sync_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_background_spawn_json")
func lingxi_ish_background_spawn_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_background_spawn_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_background_kill_json")
func lingxi_ish_background_kill_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_background_kill_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_background_poll_json")
func lingxi_ish_background_poll_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_background_poll_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_probe_loopback_json")
func lingxi_ish_probe_loopback_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_probe_loopback_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_pty_open_json")
func lingxi_ish_pty_open_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_pty_open_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_pty_write_json")
func lingxi_ish_pty_write_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_pty_write_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_pty_resize_json")
func lingxi_ish_pty_resize_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_pty_resize_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_pty_close_json")
func lingxi_ish_pty_close_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_pty_close_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_pty_poll_json")
func lingxi_ish_pty_poll_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    lx_ish_native_poll_output_json(configJSON, requestJSON)
}

@_cdecl("lingxi_ish_pty_read_json")
func lingxi_ish_pty_read_json(
    _ configJSON: UnsafePointer<CChar>?,
    _ requestJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    guard let raw = lx_ish_native_poll_output_json(configJSON, requestJSON) else { return nil }
    let result = aggregatePtyReadJSON(from: String(cString: raw))
    lx_ish_native_free_string(raw)
    return bridgeString(result)
}

@_cdecl("lingxi_ish_free_string")
func lingxi_ish_free_string(_ value: UnsafeMutablePointer<CChar>?) {
    lx_ish_native_free_string(value)
}
