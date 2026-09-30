package com.lingxi.code.cron

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.util.UUID

/**
 * Discovers managed Project workspaces without parsing or exposing project.json.
 * A malformed, symlinked or escaped Project is ignored independently.
 */
internal class CronScopeScanner(context: Context) {
    private val appContext = context.applicationContext
    private val projectsRoot = File(appContext.filesDir, "projects")

    fun scan(): List<CronScope> {
        val scopes = mutableListOf(CronScope.global(appContext))
        val canonicalProjectsRoot = runCatching { projectsRoot.canonicalFile }.getOrNull()
            ?: return scopes
        val projects = projectsRoot.listFiles()
            .orEmpty()
            .filter { it.isDirectory && isLowercaseUuid(it.name) }
            .sortedBy { it.name }
        for (directory in projects) {
            val scope = runCatching {
                require(!Files.isSymbolicLink(directory.toPath()))
                val canonicalDirectory = directory.canonicalFile
                require(canonicalDirectory.parentFile == canonicalProjectsRoot)
                val manifest = File(canonicalDirectory, "project.json")
                require(manifest.isFile)
                val project = JSONObject(manifest.readText(Charsets.UTF_8))
                require(project.getString("id") == directory.name)
                val projectName = project.getString("name").trim().also { require(it.isNotEmpty()) }
                val workspace = File(canonicalDirectory, "workspace")
                require(workspace.isDirectory)
                require(!Files.isSymbolicLink(workspace.toPath()))
                val canonicalWorkspace = workspace.canonicalFile
                require(canonicalWorkspace.parentFile == canonicalDirectory)
                CronScope(
                    scopeId = directory.name,
                    projectId = directory.name,
                    projectName = projectName,
                    workspacePath = canonicalWorkspace.path,
                    guestPath = "/workspace/${directory.name}",
                )
            }.getOrNull()
            if (scope != null) scopes += scope
        }
        return scopes
    }

    fun find(scopeId: String): CronScope? = scan().firstOrNull { it.scopeId == scopeId }

    fun activeScopeId(scopes: List<CronScope> = scan()): String {
        val index = File(projectsRoot, "index.json")
        val active = runCatching {
            val json = JSONObject(index.readText(Charsets.UTF_8))
            if (json.isNull("activeProjectId")) null else json.getString("activeProjectId")
        }.getOrNull()
        return active?.takeIf { id -> scopes.any { it.scopeId == id } } ?: GLOBAL_CRON_SCOPE_ID
    }

    private fun isLowercaseUuid(value: String): Boolean =
        value.length == 36 &&
            value == value.lowercase() &&
            runCatching { UUID.fromString(value).toString() == value }.getOrDefault(false)
}
