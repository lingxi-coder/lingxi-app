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

    fun scheduledWorkspace(context: Context, sessionId: String? = null): ProjectWorkspace {
        val paths = directories(context)
        val brandHome = File(paths.configHome)
        val relative = sessionId?.removePrefix("sess:")?.let { id ->
            require(runCatching { java.util.UUID.fromString(id).toString() == id }.getOrDefault(false))
            val manifest = File(brandHome, "automations/sessions/$id.json")
            if (!manifest.exists()) null else {
                require(!java.nio.file.Files.isSymbolicLink(manifest.toPath()))
                val record = org.json.JSONObject(manifest.readText(Charsets.UTF_8))
                require(record.getInt("version") == 1)
                record.getString("workspace").also { value ->
                    val parts = value.split('/')
                    require(parts.size == 3 && parts[0] == "workspaces" && parts.all { it.isNotBlank() && it != "." && it != ".." && !it.contains('\\') })
                }
            }
        }
        if (relative == null) return ProjectWorkspace("scheduled", paths.scheduledCwd, paths.scheduledGuestCwd)
        val root = brandHome.canonicalFile
        var workspace = brandHome
        relative.split('/').forEach { component ->
            workspace = File(workspace, component)
            require(workspace.isDirectory && !java.nio.file.Files.isSymbolicLink(workspace.toPath()))
        }
        require(workspace.canonicalFile.toPath().startsWith(File(root, "workspaces").toPath()))
        val brandGuest = paths.guestHome + "/" + File(paths.home).canonicalFile.toPath().relativize(root.toPath()).toString()
        return ProjectWorkspace("scheduled", workspace.canonicalPath, "$brandGuest/$relative")
    }
}
