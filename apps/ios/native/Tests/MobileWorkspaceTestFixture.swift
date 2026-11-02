import Foundation
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
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
func mobileWorkspaceTestLaunchConfiguration(sandboxRoot: URL, apiBase: String, model: String) throws -> IosEngineLaunchConfigFfi {
    let directories = MobileUserStorage.directories(managedRoot: sandboxRoot.appendingPathComponent("runtime").path)
    return IosEngineLaunchConfigFfi(
        appVersion: nil, appAgentHost: false,
        brandId: directories.brandId, linuxUser: directories.linuxUser,
        configDirName: directories.configDirName, nativeConfigHome: directories.configHome,
        apiBase: apiBase, model: model, sessionMode: .code, initialSessionId: nil,
        visionDelegationEnabled: false, appSandboxRoot: sandboxRoot.path, projectCwd: nil,
        projectAdditionalDirectories: [], projectMountGrants: [], providerConfig: nil,
        mobileLinux: try mobileWorkspaceTestConfiguration(sandboxRoot: sandboxRoot),
        physicalMemoryBytes: 0, hostEnvironment: nil)
}
#endif
