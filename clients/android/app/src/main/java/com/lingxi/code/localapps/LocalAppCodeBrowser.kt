package com.lingxi.code.localapps

import com.lingxi.code.R
import java.io.File
import java.nio.charset.CodingErrorAction
import java.nio.file.Files
import java.nio.file.StandardCopyOption

data class LocalAppSourceFile(val relativePath: String, val size: Long)

/**
 * A deliberately small, local-only source browser for the user-owned app workspace.
 * It never accepts an absolute path, follows a symlink, or exposes generated/private
 * directories. Generated code is still validated by Rust before the next build.
 *
 * [strings] resolves the exception messages below; it defaults to the literal
 * zh-Hans copy (see [LocalAppsStrings]) so `LocalAppCodeBrowserTest` — which
 * constructs this class directly with no Android `Context` — keeps passing
 * unmodified. The production call site ([LocalAppsScreen.kt]'s
 * `LocalAppCodeDetails`) passes [localAppsStrings] built from the real
 * `Context`, so the messages this class throws (surfaced verbatim in the code
 * editor's error banner) render in the user's selected language.
 */
class LocalAppCodeBrowser(
    appFilesRoot: File,
    workspaceRelativePath: String,
    private val strings: LocalAppsStrings = DefaultLocalAppsStrings,
) {
    private val appRoot = appFilesRoot.canonicalFile
    private val workspace = resolveWorkspace(workspaceRelativePath)

    fun listFiles(): List<LocalAppSourceFile> {
        if (!workspace.isDirectory || Files.isSymbolicLink(workspace.toPath())) return emptyList()
        return workspace.walkTopDown()
            .onEnter { directory ->
                !Files.isSymbolicLink(directory.toPath()) &&
                    (directory == workspace || directory.name !in excludedDirectories)
            }
            .filter { file ->
                file.isFile &&
                    !Files.isSymbolicLink(file.toPath()) &&
                    file.length() <= maximumEditableBytes &&
                    isEditable(file) &&
                    isDescendant(file.canonicalFile, workspace)
            }
            .map { file ->
                LocalAppSourceFile(
                    relativePath = file.relativeTo(workspace).invariantSeparatorsPath,
                    size = file.length(),
                )
            }
            .sortedBy(LocalAppSourceFile::relativePath)
            .toList()
    }

    fun read(relativePath: String): String {
        val file = resolveFile(relativePath)
        val bytes = file.readBytes()
        require(bytes.size <= maximumEditableBytes) {
            strings.resolve(R.string.local_apps_browser_error_too_large, "单个可编辑源码文件不能超过 1 MiB。")
        }
        return Charsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
            .decode(java.nio.ByteBuffer.wrap(bytes))
            .toString()
    }

    fun save(relativePath: String, text: String) {
        val bytes = text.toByteArray(Charsets.UTF_8)
        require(bytes.size <= maximumEditableBytes) {
            strings.resolve(R.string.local_apps_browser_error_too_large, "单个可编辑源码文件不能超过 1 MiB。")
        }
        val target = resolveFile(relativePath)
        val temporary = File.createTempFile(".lingxi-edit-", ".tmp", target.parentFile)
        try {
            temporary.writeBytes(bytes)
            runCatching {
                Files.move(
                    temporary.toPath(),
                    target.toPath(),
                    StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING,
                )
            }.getOrElse {
                Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.REPLACE_EXISTING)
            }
        } finally {
            temporary.delete()
        }
    }

    private fun resolveWorkspace(relativePath: String): File {
        require(relativePath.isNotBlank() && !File(relativePath).isAbsolute) {
            strings.resolve(R.string.local_apps_browser_error_workspace_absolute, "已拒绝绝对工作区路径。")
        }
        val normalized = relativePath.replace('\\', '/').trim('/')
        val segments = normalized.split('/')
        require(
            segments.size == 3 && segments[0] == "apps" &&
                appId.matches(segments[1]) && segments[2] == "workspace",
        ) { strings.resolve(R.string.local_apps_browser_error_workspace_invalid, "已拒绝无效工作区路径。") }
        val candidate = File(appRoot, normalized).canonicalFile
        require(isDescendant(candidate, appRoot)) {
            strings.resolve(R.string.local_apps_browser_error_path_escaped, "已拒绝工作区之外的路径。")
        }
        return candidate
    }

    private fun resolveFile(relativePath: String): File {
        require(relativePath.isNotBlank() && !File(relativePath).isAbsolute) {
            strings.resolve(R.string.local_apps_browser_error_source_absolute, "已拒绝绝对源码路径。")
        }
        val candidate = File(workspace, relativePath).canonicalFile
        require(isDescendant(candidate, workspace)) {
            strings.resolve(R.string.local_apps_browser_error_path_escaped, "已拒绝工作区之外的路径。")
        }
        require(candidate.isFile && !Files.isSymbolicLink(candidate.toPath()) && isEditable(candidate)) {
            strings.resolve(R.string.local_apps_browser_error_not_editable, "该文件不是可编辑的 UTF-8 源码。")
        }
        return candidate
    }

    private fun isEditable(file: File): Boolean =
        file.name in editableNames || file.extension.lowercase() in editableExtensions

    private fun isDescendant(candidate: File, root: File): Boolean =
        candidate.path == root.path || candidate.path.startsWith(root.path + File.separator)

    private companion object {
        const val maximumEditableBytes = 1_048_576
        val appId = Regex("^[a-z0-9][a-z0-9-]{0,63}$")
        val excludedDirectories = setOf(".git", ".lingxi", ".next", "build", "node_modules")
        val editableExtensions = setOf(
            "css", "html", "js", "json", "jsx", "md", "mjs", "svg", "ts", "tsx", "txt",
        )
        val editableNames = setOf(
            ".gitignore", ".npmrc", "next.config.js", "next.config.mjs", "pnpm-lock.yaml", "pnpm-workspace.yaml", "package.json",
        )
    }
}
