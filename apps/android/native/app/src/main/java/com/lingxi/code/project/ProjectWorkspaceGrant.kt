package com.lingxi.code.project

import java.io.File
import java.nio.file.Files
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.flow.first

/** SDK-derived roots used to validate an explicit Project directory grant. */
internal data class ProjectGrantRoots(
    val home: File,
    val guestHome: String,
    val configDirName: String,
    val configGuestPath: String,
    val chatGuestCwd: String,
    val codeGuestCwd: String,
    val scheduledGuestCwd: String,
    val appsGuestPath: String,
    val projectsGuestPath: String,
)

internal data class AuthoringSourceIdentity(
    val appId: String?,
    val projectId: String,
    val hostPath: String,
    val guestPath: String,
)

/** A trusted create/prepare result authorizes only one still-current navigation. */
internal class PreparedAuthoringGrant(
    internal val identity: AuthoringSourceIdentity,
    private val navigationGeneration: Long,
) {
    private var consumed = false

    @Synchronized fun consume(expected: AuthoringSourceIdentity, generation: Long, stillCurrent: Boolean): PendingAuthoringAdmission? {
        if (consumed || !stillCurrent || generation != navigationGeneration || expected != identity) return null
        consumed = true
        return PendingAuthoringAdmission(identity)
    }
}

/** Owner-local gate opened only after an exact durable authoring binding is acknowledged. */
internal class PendingAuthoringAdmission(internal val identity: AuthoringSourceIdentity) {
    private val readyState = kotlinx.coroutines.flow.MutableStateFlow(false)
    val ready: Boolean get() = readyState.value

    suspend fun awaitReady() = readyState.first { it }

    @Synchronized fun acknowledge(bound: AuthoringSourceIdentity, sourceSessionId: String, bindingSessionId: String): Boolean {
        if (!sameAuthoringSource(bound, identity) || sourceSessionId.isBlank() || sourceSessionId != bindingSessionId) return false
        readyState.value = true
        return true
    }
}

/** App ID is presentation metadata; the immutable source identity is this exact tuple. */
internal fun sameAuthoringSource(a: AuthoringSourceIdentity, b: AuthoringSourceIdentity): Boolean =
    a.projectId == b.projectId && a.hostPath == b.hostPath && a.guestPath == b.guestPath

internal fun scheduledPickerPathAllowed(scheduled: Boolean, workspace: ProjectWorkspace?, path: String): Boolean {
    if (!scheduled) return true
    val root = workspace?.takeIf { it.projectId == "scheduled" }?.guestPath?.trimEnd('/') ?: return false
    return root.isNotBlank() && (path == root || path.startsWith("$root/"))
}

/**
 * Check a user-selected Project grant against actual directories and the exact
 * host/guest pair. A managed project root comes only from its validated
 * ProjectSnapshot; an app source needs a separately verified authoring receipt.
 */
internal fun isSafeProjectWorkspace(
    roots: ProjectGrantRoots,
    guestPath: String,
    expectedHostPath: String?,
    project: ProjectSnapshot?,
    verifiedAppSource: AuthoringSourceIdentity? = null,
    preparedAppDirectory: String? = null,
    verifiedScheduled: ProjectWorkspace? = null,
): Boolean = runCatching {
    val prefix = roots.guestHome.trimEnd('/') + "/"
    require(guestPath.startsWith(prefix) && guestPath != roots.guestHome)
    val components = guestPath.removePrefix(prefix).split('/')
    require(components.all { it.isNotBlank() && it != "." && it != ".." && '\u0000' !in it && '\\' !in it })
    require(!Files.isSymbolicLink(roots.home.toPath()))
    val home = roots.home.canonicalFile
    var target = home
    components.forEach { component ->
        target = File(target, component)
        require(!Files.isSymbolicLink(target.toPath()) && target.isDirectory)
    }
    val actual = target.canonicalFile
    require(actual.toPath().startsWith(home.toPath()) && actual != home)
    expectedHostPath?.let { require(File(it).absoluteFile.toPath().normalize() == actual.toPath()) }

    fun within(path: String, root: String): Boolean = path == root || path.startsWith("$root/")
    val inConfig = within(guestPath, roots.configGuestPath)
    val checked = if (inConfig) components.drop(1) else components
    require(checked.none { it.equals(roots.configDirName, true) || it.equals("provider", true) || it.equals("providers", true) })
    require(checked.zipWithNext().none { (a, b) -> a.equals("Library", true) && b.equals("Preferences", true) })
    if (inConfig) {
        val ownedProjectRoot = project?.let { snapshot ->
            require(snapshot.sourceWorkspace.projectId == snapshot.record.id)
            val source = File(snapshot.sourceWorkspace.hostPath).absoluteFile.toPath().normalize()
            val expected = File(File(File(home, roots.projectsGuestPath.removePrefix(prefix)).canonicalFile,
                snapshot.record.id), PROJECT_WORKSPACE_DIR).absoluteFile.toPath().normalize()
            require(source == expected && source.startsWith(home.toPath()))
            roots.guestHome.trimEnd('/') + "/" + home.toPath().relativize(source).joinToString("/")
        }
        val appRoot = verifiedAppSource?.takeIf { proof ->
            project != null && proof.projectId == project.record.id &&
                File(proof.hostPath).absoluteFile.toPath().normalize() ==
                File(home, proof.guestPath.removePrefix(prefix)).absoluteFile.toPath().normalize() &&
                proof.guestPath.startsWith(roots.appsGuestPath.trimEnd('/') + "/")
        }?.guestPath
        require(within(guestPath, roots.chatGuestCwd) || within(guestPath, roots.codeGuestCwd) ||
            (project != null && guestPath != roots.scheduledGuestCwd && within(guestPath, roots.scheduledGuestCwd)) ||
            (ownedProjectRoot != null && within(guestPath, ownedProjectRoot)) ||
            (appRoot != null && within(guestPath, appRoot)) ||
            (verifiedScheduled?.projectId == "scheduled" && within(guestPath, verifiedScheduled.guestPath)) ||
            (preparedAppDirectory == guestPath && guestPath.startsWith(roots.appsGuestPath.trimEnd('/') + "/") &&
                guestPath.removePrefix(roots.appsGuestPath.trimEnd('/') + "/").let { it.isNotEmpty() && '/' !in it }))
    }
    true
}.getOrDefault(false)

internal fun projectWorkspaceHasValidGrant(
    roots: ProjectGrantRoots,
    project: ProjectSnapshot,
    workspace: ProjectWorkspace,
    authoring: AuthoringSourceIdentity?,
): Boolean = workspace.projectId == project.record.id &&
    isSafeProjectWorkspace(roots, workspace.guestPath, workspace.hostPath, project, authoring) &&
    workspace.additionalDirectories.all { path -> isSafeProjectWorkspace(roots, path, null, project, authoring) }

internal fun retainedAuthoringSourceMatches(
    expected: AuthoringSourceIdentity,
    sessionId: String,
    mode: SessionMode,
    activeSessionId: String?,
    workspace: SessionWorkspaceState?,
    sourceTranscript: String?,
    sourceHostPath: String?,
    sourceGuestPath: String?,
    sourceProjectId: String?,
    sourceWorkspaceKey: String?,
    sourceMode: SessionMode?,
): Boolean = mode == SessionMode.Code && activeSessionId != null && workspace != null &&
    sourceMode == mode && sourceProjectId == expected.projectId &&
    sourceWorkspaceKey == "project.${expected.projectId}" &&
    sourceHostPath == expected.hostPath && sourceGuestPath == expected.guestPath &&
    canonicalSessionId(activeSessionId) == canonicalSessionId(sessionId) &&
    canonicalSessionId(workspace.sessionId) == canonicalSessionId(sessionId) &&
    workspace.cwd == expected.guestPath && workspace.transcriptPath.isNotBlank() &&
    workspace.transcriptPath == sourceTranscript

internal data class AuthoringGrantReceipt(
    val sourceDirectory: String,
    val projectId: String,
    val sessionId: String,
    val workspaceId: String,
    val appId: String?,
)

/** The second read fences changes to the selected Project while the receipt lookup suspends. */
internal suspend fun verifyProjectSource(
    roots: ProjectGrantRoots,
    workspaceKey: String,
    workspace: ProjectWorkspace,
    mode: SessionMode,
    sessionId: String?,
    pending: PendingAuthoringAdmission?,
    loadProject: suspend () -> ProjectSnapshot,
    ownerMatches: suspend (String, String, SessionMode) -> Boolean,
    lookupAuthoring: suspend (String, String, String) -> AuthoringGrantReceipt,
): AuthoringSourceIdentity? {
    fun canonicalUuid(value: String): Boolean = runCatching { java.util.UUID.fromString(value).toString() == value }.getOrDefault(false)
    val projectId = workspaceKey.removePrefix("project.")
    require(workspaceKey == "project.$projectId" && canonicalUuid(projectId) && workspace.projectId == projectId)
    val project = loadProject()
    require(project.record.id == projectId)
    val canonicalId = sessionId?.let(::canonicalSessionId)
    if (canonicalId != null) {
        require(canonicalUuid(canonicalId))
        require(project.sessions.any { canonicalSessionId(it.sessionId) == canonicalId && it.mode == mode })
        require(ownerMatches(canonicalId, workspaceKey, mode))
    }
    val appPrefix = roots.appsGuestPath.trimEnd('/') + "/"
    val needsAppGrant = (listOf(workspace.guestPath) + workspace.additionalDirectories).any { it.startsWith(appPrefix) }
    if (pending != null || needsAppGrant) com.lingxi.code.apps.requireCodeAuthoring(mode)
    val identity = if (pending != null) {
        pending.identity.also { source ->
            require(mode == SessionMode.Code && source.projectId == projectId && source.guestPath == project.record.directory)
            require(source.hostPath == File(roots.home, source.guestPath.removePrefix(roots.guestHome + "/")).canonicalPath)
        }
    } else if (needsAppGrant) {
        require(mode == SessionMode.Code && canonicalId != null)
        val source = project.record.directory
        require(source.startsWith(appPrefix) && source.removePrefix(appPrefix).let { it.isNotBlank() && '/' !in it })
        val receipt = lookupAuthoring(source, projectId, canonicalId)
        require(receipt.sourceDirectory == source && receipt.projectId == projectId &&
            canonicalSessionId(receipt.sessionId) == canonicalId && canonicalUuid(receipt.workspaceId))
        AuthoringSourceIdentity(receipt.appId, projectId,
            File(roots.home, source.removePrefix(roots.guestHome + "/")).canonicalPath, source)
    } else null
    require(projectWorkspaceHasValidGrant(roots, project, workspace, identity))
    require(loadProject() == project)
    return identity
}
