package com.lingxi.code.project

import android.content.Context
import com.lingxi.code.bindings.runtime.MobileUserDirectories
import com.lingxi.code.bindings.runtime.mobileUserDirectories
import com.lingxi.code.settings.LinuxRuntimeBridge
import java.io.File

/** Paths owned by the Linux runtime user, resolved by the shared Harness. */
object MobileUserStorage {
    fun directories(context: Context): MobileUserDirectories =
        mobileUserDirectories(LinuxRuntimeBridge.managedRoot(context))

    fun projects(context: Context): File = File(directories(context).projects)

    fun scheduledWorkspace(context: Context): ProjectWorkspace {
        val paths = directories(context)
        return ProjectWorkspace("scheduled", paths.scheduledCwd, paths.scheduledGuestCwd)
    }
}
