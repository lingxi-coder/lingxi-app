package com.lingxi.code.project

import android.content.Context
import com.lingxi.code.apps.AppServiceException
import com.lingxi.code.apps.AuthoringIntent
import com.lingxi.code.apps.AuthoringIntentStore
import com.lingxi.code.apps.AppProjectRef
import com.lingxi.code.apps.matchesOwner
import com.lingxi.code.apps.UnifiedLocalApps
import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File

internal data class AdmittedProjectSource(val identity: AuthoringSourceIdentity?, val recovery: AuthoringIntent?)

/** A missing receipt permits only a gated native resume, never a pre-resume registration. */
internal suspend fun admitProjectSource(
    context: Context,
    workspaceKey: String,
    workspace: ProjectWorkspace,
    mode: SessionMode,
    sessionId: String?,
    pending: PendingAuthoringAdmission?,
    transcriptPath: String?,
): AdmittedProjectSource = withContext(Dispatchers.IO) {
    val paths = MobileUserStorage.directories(context)
    val roots = paths.projectGrantRoots()
    val projectId = workspaceKey.removePrefix("project.")
    val repository = ProjectRepository(
        File(paths.projects), userHome = File(paths.home), projectsGuestRoot = paths.projectsGuestPath,
        userHomeGuestPath = paths.guestHome, ownerStateFile = File(context.filesDir, ScopeStateStore.SCOPE_STATE_FILE),
        strings = projectStrings(context),
    )
    suspend fun snapshot(): ProjectSnapshot = ProjectStore.admissionRepositoryMutex.withLock {
        repository.load().projects.single { it.record.id == projectId }
    }
    var recovery: AuthoringIntent? = null
    val identity = verifyProjectSource(roots, workspaceKey, workspace, mode, sessionId, pending,
        loadProject = { snapshot() },
        ownerMatches = { id, key, ownerMode ->
            ScopeStateStore(context).readSessionOwners()[id] == ScopeStateStore.SessionOwner(key, ownerMode)
        },
        lookupAuthoring = { source, id, session ->
            val apps = UnifiedLocalApps.get(context)
            try {
                val receipt = apps.getAuthoringGrant(source, id, session, mode)
                AuthoringIntentStore(context).read(session)?.let { intent ->
                    require(intent.sessionMode == mode && intent.projectId == id && intent.sourceDirectory == source &&
                        receipt.matchesOwner(intent.sourceId, source, id, intent.workspaceId, session)) {
                        "Authoring receipt conflicts with the saved source"
                    }
                }
                val binding = receipt.authoring
                require(binding.projectRef == AppProjectRef("mobileProject", projectId = id))
                AuthoringGrantReceipt(binding.sourceDirectory, id, checkNotNull(binding.authoringSessionId),
                    checkNotNull(binding.workspaceId), binding.appId)
            } catch (error: AppServiceException) {
                if (error.code != "notFound") throw error
                val intent = checkNotNull(AuthoringIntentStore(context).read(session)) { "Authoring source intent is unavailable" }
                require(mode == SessionMode.Code && intent.sessionMode == mode && intent.sessionId == session &&
                    intent.projectId == id && intent.sourceDirectory == source && !transcriptPath.isNullOrBlank() &&
                    intent.transcriptPath == transcriptPath)
                val proof = apps.prepareAuthoringSource(intent.sourceId, source, intent.sourceRequestId, mode)
                require(proof.sourceId == intent.sourceId && proof.path == source &&
                    proof.sourcePath == apps.hostFile(source).absolutePath)
                recovery = intent
                AuthoringGrantReceipt(source, id, session, intent.workspaceId, intent.appId)
            }
        })
    AdmittedProjectSource(identity, recovery)
}
