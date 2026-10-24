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
#endif
