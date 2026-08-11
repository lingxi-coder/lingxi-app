package com.lingxi.code.localapps

import com.lingxi.code.project.ProjectWorkspace
import java.io.File

/** The engine's app-id contract (`AppRecordDto.id`). */
private val localAppIdPattern = Regex("^[a-z0-9][a-z0-9-]{0,63}$")

/**
 * Resolve a local app's conversation workspace against the engine data root —
 * `Context.filesDir`, the exact root [LocalAppCodeBrowser] and the runtime
 * distribution resolve the record's `workspace_rel` against. The wire contract
 * pins `workspace_rel` to `apps/<id>/workspace`; anything else (or a missing
 * rel, e.g. before the record loaded) falls back to that canonical shape
 * rather than trusting a stray path near the sandbox root.
 *
 * Returned as a [ProjectWorkspace] because that is the shape
 * `EngineConversationSource.create` / `buildVoiceEngine` already accept as the
 * session cwd (`projectCwd = workspace.hostPath`); `projectId = appId` keys
 * the guest bind path exactly like a project does.
 */
fun localAppWorkspace(appFilesRoot: File, appId: String, workspaceRel: String?): ProjectWorkspace {
    val rel = workspaceRel?.replace('\\', '/')?.trim('/')
        ?.takeIf { candidate ->
            val segments = candidate.split('/')
            segments.size == 3 && segments[0] == "apps" &&
                localAppIdPattern.matches(segments[1]) && segments[2] == "workspace"
        }
        ?: "apps/$appId/workspace"
    val directory = File(appFilesRoot, rel)
    if (!directory.exists()) directory.mkdirs()
    return ProjectWorkspace(projectId = appId, hostPath = directory.absolutePath)
}
