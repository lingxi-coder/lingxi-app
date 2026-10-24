package com.lingxi.code

import android.os.Build
import com.lingxi.code.bindings.android.AndroidMobileLinuxConfigFfi
import com.lingxi.code.bindings.android.MobileLinuxRuntimeModeFfi
import com.lingxi.code.bindings.runtime.mobileUserDirectories
import java.io.File

/** An isolated Linux account layout; constructing it does not boot a guest. */
internal fun mobileWorkspaceTestConfiguration(root: File): AndroidMobileLinuxConfigFfi {
    val managedRoot = File(root, "runtime").path
    val directories = mobileUserDirectories(managedRoot)
    return AndroidMobileLinuxConfigFfi(
        mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
        brandId = directories.brandId,
        linuxUser = directories.linuxUser,
        configDirName = directories.configDirName,
        managedRoot = managedRoot,
        appSandboxRoot = root.path,
        workspaceHostPath = directories.projects,
        stableWorkspaceId = "projects",
        abi = Build.SUPPORTED_ABIS.firstOrNull() ?: "arm64-v8a",
        rootfsVersion = "test",
        archiveSha256 = null,
    )
}
