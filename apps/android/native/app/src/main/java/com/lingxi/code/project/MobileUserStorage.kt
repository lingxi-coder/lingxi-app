package com.lingxi.code.project

import android.content.Context
import com.lingxi.code.bindings.runtime.MobileUserDirectories
import com.lingxi.code.bindings.runtime.mobileUserDirectories
import com.lingxi.code.model.SessionMode
import com.lingxi.code.settings.LinuxRuntimeBridge
import java.io.File
import java.io.FileDescriptor
import java.io.FileInputStream

private inline fun <T> withOpenDescriptor(fd: FileDescriptor, block: (FileDescriptor) -> T): T =
    try { block(fd) } finally { if (fd.valid()) android.system.Os.close(fd) }

/** Resolve each component relative to a held directory, never a replaceable parent path. */
private fun openAnchoredChild(parent: FileDescriptor, name: String, flags: Int): FileDescriptor =
    android.os.ParcelFileDescriptor.dup(parent).use { anchor ->
        android.system.Os.open("/proc/self/fd/${anchor.fd}/$name", flags, 0)
    }

private fun readScheduledManifest(brandHome: File, sessionId: String): String? {
    val directoryFlags = android.system.OsConstants.O_RDONLY or android.system.OsConstants.O_NONBLOCK or
        android.system.OsConstants.O_NOFOLLOW or android.system.OsConstants.O_CLOEXEC
    val fileFlags = android.system.OsConstants.O_RDONLY or android.system.OsConstants.O_NONBLOCK or
        android.system.OsConstants.O_NOFOLLOW or android.system.OsConstants.O_CLOEXEC
    return try {
        withOpenDescriptor(android.system.Os.open(brandHome.absolutePath, directoryFlags, 0)) { brand ->
            require(android.system.OsConstants.S_ISDIR(android.system.Os.fstat(brand).st_mode))
            withOpenDescriptor(openAnchoredChild(brand, "automations", directoryFlags)) { automations ->
                require(android.system.OsConstants.S_ISDIR(android.system.Os.fstat(automations).st_mode))
                withOpenDescriptor(openAnchoredChild(automations, "sessions", directoryFlags)) { sessions ->
                    require(android.system.OsConstants.S_ISDIR(android.system.Os.fstat(sessions).st_mode))
                    withOpenDescriptor(openAnchoredChild(sessions, "$sessionId.json", fileFlags)) { file ->
                        require(android.system.OsConstants.S_ISREG(android.system.Os.fstat(file).st_mode)) {
                            "Scheduled session manifest is not a regular file"
                        }
                        FileInputStream(file).bufferedReader(Charsets.UTF_8).use { it.readText() }
                    }
                }
            }
        }
    } catch (failure: android.system.ErrnoException) {
        if (failure.errno == android.system.OsConstants.ENOENT) null else throw failure
    }
}

/** A Global workspace is selected by the session's immutable mode. */
internal fun MobileUserDirectories.globalGuestCwd(mode: SessionMode): String = when (mode) {
    SessionMode.Chat -> chatGuestCwd
    SessionMode.Code -> codeGuestCwd
}

internal fun MobileUserDirectories.projectGrantRoots(): ProjectGrantRoots = ProjectGrantRoots(
    File(home), guestHome, configDirName, configGuestPath, chatGuestCwd, codeGuestCwd,
    scheduledGuestCwd, appsGuestPath, projectsGuestPath,
)

/** SDK-owned content roots, separate from the private configuration container. */
internal fun MobileUserDirectories.workspaceDirectoryRepository(
    workspace: String,
    verifiedProjects: List<ProjectSnapshot> = emptyList(),
    verifiedScheduled: ProjectWorkspace? = null,
    verifiedAppSource: AuthoringSourceIdentity? = null,
): WorkspaceDirectoryRepository {
    val canonicalHome = File(home).canonicalFile.toPath()
    val projectRoots = verifiedProjects.mapNotNull { snapshot ->
        runCatching {
            require(snapshot.record.id == snapshot.sourceWorkspace.projectId)
            require(java.util.UUID.fromString(snapshot.record.id).toString() == snapshot.record.id)
            val source = File(snapshot.sourceWorkspace.hostPath).absoluteFile.toPath().normalize()
            val expected = File(File(File(projects).canonicalFile, snapshot.record.id), PROJECT_WORKSPACE_DIR)
                .absoluteFile.toPath().normalize()
            require(source == expected)
            require(source.startsWith(canonicalHome) && source != canonicalHome)
            guestHome.trimEnd('/') + "/" + canonicalHome.relativize(source).joinToString("/")
        }.getOrNull()
    }
    val scheduledRoots = verifiedScheduled?.takeIf { it.projectId == "scheduled" }
        ?.guestPath?.takeIf(String::isNotBlank)?.let(::listOf).orEmpty()
    val appRoots = verifiedAppSource?.let { source ->
        val project = verifiedProjects.singleOrNull { it.record.id == source.projectId && it.record.directory == source.guestPath }
        source.guestPath.takeIf { project != null &&
            isSafeProjectWorkspace(projectGrantRoots(), it, source.hostPath, project, source) }
    }?.let(::listOf).orEmpty()
    return WorkspaceDirectoryRepository(
        File(home), guestHome, workspace, projectsGuestPath, File(configHome),
        listOf(chatGuestCwd, codeGuestCwd) + projectRoots + scheduledRoots + appRoots,
    )
}

/** Paths owned by the Linux runtime user, resolved by the shared Harness. */
object MobileUserStorage {
    fun directories(context: Context): MobileUserDirectories =
        mobileUserDirectories(LinuxRuntimeBridge.managedRoot(context))

    fun projects(context: Context): File = File(directories(context).projects)

    /** Only a present, validated per-session manifest can grant a task content root. */
    fun verifiedScheduledWorkspace(context: Context, sessionId: String): ProjectWorkspace? {
        val id = sessionId.removePrefix("sess:")
        if (!runCatching { java.util.UUID.fromString(id).toString() == id }.getOrDefault(false)) return null
        val brand = File(directories(context).configHome)
        val candidate = runCatching { scheduledWorkspace(context, id) }.getOrNull() ?: return null
        val expectedContainer = File(brand.canonicalFile, "workspaces").toPath().toAbsolutePath().normalize()
        val actual = File(candidate.hostPath).toPath().toAbsolutePath().normalize()
        return candidate.takeIf { actual != expectedContainer && actual.startsWith(expectedContainer) }
    }

    fun scheduledWorkspace(context: Context, sessionId: String? = null): ProjectWorkspace {
        val paths = directories(context)
        val brandHome = File(paths.configHome)
        val relative = sessionId?.removePrefix("sess:")?.let { id ->
            require(runCatching { java.util.UUID.fromString(id).toString() == id }.getOrDefault(false))
            val data = readScheduledManifest(brandHome, id)
            if (data == null) null else {
                val record = org.json.JSONObject(data)
                require(record.get("version") == 1 && record.get("task_id") is String)
                val workspace = record.get("workspace") as? String ?: error("Invalid scheduled workspace manifest")
                workspace.also { value ->
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
