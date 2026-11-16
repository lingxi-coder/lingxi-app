import Foundation
import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

/// iSH owns one immutable kernel configuration per process, including its
/// managed/app roots and protected config path. Individual tests therefore
/// share only that kernel identity; Projects and mutable account/session data
/// are isolated and cleaned after every handle has released its native owner.
final class MobileWorkspaceNativeTestFixture: @unchecked Sendable {
    private final class ProcessKernel: @unchecked Sendable {
        static let shared = ProcessKernel()
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("native-kernel-\(UUID().uuidString)", isDirectory: true)
            .resolvingSymlinksInPath()
        private let lock = NSLock()
        private var owner: UUID?

        func claim(_ id: UUID) throws {
            lock.lock(); defer { lock.unlock() }
            guard owner == nil else {
                throw NSError(domain: "MobileWorkspaceNativeTestFixture", code: 1,
                    userInfo: [NSLocalizedDescriptionKey:
                        "Another native fixture still owns the process kernel; run cases serially and await teardown."])
            }
            owner = id
        }

        func release(_ id: UUID) {
            lock.lock(); defer { lock.unlock() }
            precondition(owner == id, "Only the active fixture may release the process kernel")
            owner = nil
        }
    }

    private final class WeakHandle {
        weak var value: MobileEngineHandle?
        init(_ value: MobileEngineHandle) { self.value = value }
    }

    let sandboxRoot: URL
    let directories: MobileUserDirectories
    let caseDirectory: URL
    private let caseID: String
    private let ownerID: UUID
    private let handlesLock = NSLock()
    private var handles: [WeakHandle] = []

    init(testCase: XCTestCase) throws {
        let kernel = ProcessKernel.shared
        let id = UUID()
        try kernel.claim(id)
        ownerID = id
        sandboxRoot = kernel.root
        caseID = "fixture-\(id.uuidString.lowercased())"
        directories = MobileUserStorage.directories(
            managedRoot: sandboxRoot.appendingPathComponent("runtime", isDirectory: true).path)
        caseDirectory = sandboxRoot.appendingPathComponent("cases/\(caseID)", isDirectory: true)
        do {
            for path in [directories.configHome, directories.projects, directories.chatCwd,
                         directories.codeCwd, directories.scheduledCwd, directories.apps] {
                try FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
            }
            guard try FileManager.default.contentsOfDirectory(atPath: directories.configHome).isEmpty else {
                throw NSError(domain: "MobileWorkspaceNativeTestFixture", code: 4,
                    userInfo: [NSLocalizedDescriptionKey:
                        "A previous native fixture left mutable configuration behind; refusing cross-case state."])
            }
            try FileManager.default.createDirectory(at: caseDirectory, withIntermediateDirectories: true)
            for mode in [SessionModeDto.chat, .code] {
                try FileManager.default.createDirectory(at: workspace(mode), withIntermediateDirectories: true)
            }
        } catch {
            kernel.release(ownerID)
            throw error
        }
        testCase.addTeardownBlock { [self] in try await finish() }
    }

    func workspace(_ mode: SessionModeDto = .code) -> URL {
        URL(fileURLWithPath: mode == .chat ? directories.chatCwd : directories.codeCwd,
            isDirectory: true).appendingPathComponent(caseID, isDirectory: true)
    }

    func guestCwd(_ mode: SessionModeDto = .code) -> String {
        "\(mode == .chat ? directories.chatGuestCwd : directories.codeGuestCwd)/\(caseID)"
    }

    func extraWorkspace(_ name: String) throws -> (host: URL, guest: String) {
        let host = URL(fileURLWithPath: directories.home, isDirectory: true)
            .appendingPathComponent(caseID, isDirectory: true).appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: host, withIntermediateDirectories: true)
        return (host, "\(directories.guestHome)/\(caseID)/\(name)")
    }

    func launchConfiguration(apiBase: String, model: String, sessionMode: SessionModeDto = .code,
                             initialSessionId: String? = nil, initialSessionTranscriptPath: String? = nil,
                             schedulerController: Bool = false, projectCwd: String? = nil) throws -> IosEngineLaunchConfigFfi {
        try mobileWorkspaceTestLaunchConfiguration(sandboxRoot: sandboxRoot, apiBase: apiBase, model: model,
            sessionMode: sessionMode, initialSessionId: initialSessionId,
            initialSessionTranscriptPath: initialSessionTranscriptPath, schedulerController: schedulerController,
            projectCwd: projectCwd ?? (schedulerController ? nil : guestCwd(sessionMode)))
    }

    @discardableResult
    func track(_ handle: MobileEngineHandle) -> MobileEngineHandle {
        handlesLock.lock(); defer { handlesLock.unlock() }
        handles.append(WeakHandle(handle))
        return handle
    }

    private func liveHandleCount() -> Int {
        handlesLock.lock(); defer { handlesLock.unlock() }
        return handles.filter { $0.value != nil }.count
    }

    private func finish() async throws {
        let deadline = Date().addingTimeInterval(10)
        while liveHandleCount() != 0 && Date() < deadline {
            try await Task.sleep(for: .milliseconds(25))
        }
        guard liveHandleCount() == 0 else {
            throw NSError(domain: "MobileWorkspaceNativeTestFixture", code: 2,
                userInfo: [NSLocalizedDescriptionKey:
                    "A native engine handle outlived its case; mutable kernel data was retained to prevent cross-case writes."])
        }
        let configHome = URL(fileURLWithPath: directories.configHome, isDirectory: true)
        let sources = FileManager.default.enumerator(at: configHome, includingPropertiesForKeys: nil)?
            .allObjects.compactMap { $0 as? URL } ?? []
        let sessionIDs = Set(sources.filter { $0.pathExtension == "jsonl" }
            .map { $0.deletingPathExtension().lastPathComponent }.filter { UUID(uuidString: $0) != nil })
        for id in sessionIDs {
            guard await waitForMobileSessionRelease(lingxiHome: directories.configHome,
                                                    sessionId: id, timeoutMs: 8_000) else {
                throw NSError(domain: "MobileWorkspaceNativeTestFixture", code: 3,
                    userInfo: [NSLocalizedDescriptionKey:
                        "A native session owner did not drain; its configuration and source files were not removed."])
            }
        }
        // Retain every runtime-owned root inode and the immutable process root.
        // Only disposable contents of this test account are removed.
        for path in [directories.configHome, directories.projects, directories.chatCwd,
                     directories.codeCwd, directories.scheduledCwd, directories.apps] {
            let directory = URL(fileURLWithPath: path, isDirectory: true)
            for child in try FileManager.default.contentsOfDirectory(at: directory,
                includingPropertiesForKeys: nil) {
                try FileManager.default.removeItem(at: child)
            }
        }
        let extras = URL(fileURLWithPath: directories.home, isDirectory: true)
            .appendingPathComponent(caseID, isDirectory: true)
        if FileManager.default.fileExists(atPath: extras.path) {
            try FileManager.default.removeItem(at: extras)
        }
        try FileManager.default.removeItem(at: caseDirectory)
        ProcessKernel.shared.release(ownerID)
    }
}

/// A real, isolated Linux account layout without booting a guest or using app data.
func mobileWorkspaceTestConfiguration(sandboxRoot: URL) throws -> IosMobileLinuxConfigFfi {
    let managedRoot = sandboxRoot.appendingPathComponent("runtime", isDirectory: true).path
    let paths = try mobileUserDirectories(managedRoot: managedRoot)
    return IosMobileLinuxConfigFfi(
        mode: .mobileLinux, brandId: paths.brandId, linuxUser: paths.linuxUser,
        configDirName: paths.configDirName, managedRoot: managedRoot, workspaceHostPath: paths.projects,
        stableWorkspaceId: "projects", abi: "arm64", rootfsVersion: "test",
        archiveSha256: nil, authorizationFile: nil, appSandboxRoot: sandboxRoot.path)
}
/// Current structured engine launch contract for isolated native tests.
func mobileWorkspaceTestLaunchConfiguration(sandboxRoot: URL, apiBase: String, model: String,
                                            sessionMode: SessionModeDto = .code,
                                            initialSessionId: String? = nil,
                                            initialSessionTranscriptPath: String? = nil,
                                            schedulerController: Bool = false,
                                            projectCwd: String? = nil) throws -> IosEngineLaunchConfigFfi {
    let directories = MobileUserStorage.directories(managedRoot: sandboxRoot.appendingPathComponent("runtime").path)
    return IosEngineLaunchConfigFfi(
        appVersion: nil, appAgentHost: false,
        brandId: directories.brandId, linuxUser: directories.linuxUser,
        configDirName: directories.configDirName, nativeConfigHome: directories.configHome,
        apiBase: apiBase, model: model, sessionMode: sessionMode, initialSessionId: initialSessionId,
        initialSessionTranscriptPath: initialSessionTranscriptPath, schedulerController: schedulerController,
        visionDelegationEnabled: false, appSandboxRoot: sandboxRoot.path, projectCwd: projectCwd,
        projectAdditionalDirectories: [], projectMountGrants: [], providerConfig: nil,
        mobileLinux: try mobileWorkspaceTestConfiguration(sandboxRoot: sandboxRoot),
        physicalMemoryBytes: 0, hostEnvironment: nil)
}
#endif
