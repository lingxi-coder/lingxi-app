package com.lingxi.code.settings

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.nio.charset.StandardCharsets
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.FileAlreadyExistsException
import java.nio.file.StandardCopyOption

/**
 * Reads the last successful engine selection, falling back to legacy
 * `permissions.defaultMode`. The engine owns last-selection persistence;
 * offline callers can still update the legacy default through [save].
 *
 * The Rust loader owns precedence and safety gating; this repository only
 * edits the app-private user tier and deliberately preserves every unrelated
 * key in settings.json. Writes use a sibling temp file plus rename so a
 * process death cannot leave a partially-written settings document.
 */
class PermissionModeSettingsRepository(context: Context) {
    private val directories = com.lingxi.code.project.MobileUserStorage.directories(context.applicationContext)
    private val settingsDir = File(directories.configHome)
    private val legacySettingsDir = permissionModeLegacySettingsDirectory(
        context.applicationContext.filesDir,
        directories.brandId,
        directories.linuxUser,
        directories.configDirName,
    )
    private val settingsFile = File(settingsDir, "settings.json")
    private val lastModeFile = File(settingsDir, "last-permission-mode.json")
    private val lock = Any()

    init {
        legacySettingsDir?.let { legacy -> runCatching { copyPermissionSettingsIfMissing(legacy, settingsDir) } }
    }

    fun load(): String = synchronized(lock) {
        PermissionModeSettingsJson.load(
            raw = runCatching { readRoot()?.toString() }.getOrNull(),
            lastSelection = runCatching { lastModeFile.readText(StandardCharsets.UTF_8) }.getOrNull(),
        )
    }

    fun save(mode: String) {
        require(mode in PermissionModeOptions.values) { "unknown permission mode: $mode" }
        synchronized(lock) {
            val updated = PermissionModeSettingsJson.update(readRoot()?.toString(), mode)
            settingsDir.mkdirs()
            val tmp = File(settingsDir, "settings.json.tmp")
            tmp.writeText(updated, StandardCharsets.UTF_8)
            try {
                try {
                    Files.move(
                        tmp.toPath(),
                        settingsFile.toPath(),
                        StandardCopyOption.ATOMIC_MOVE,
                        StandardCopyOption.REPLACE_EXISTING,
                    )
                } catch (_: AtomicMoveNotSupportedException) {
                    // Some Android filesystems do not expose ATOMIC_MOVE;
                    // retain the same-directory replacement semantics there.
                    Files.move(
                        tmp.toPath(),
                        settingsFile.toPath(),
                        StandardCopyOption.REPLACE_EXISTING,
                    )
                }
            } catch (error: Exception) {
                tmp.delete()
                throw error
            }
        }
    }

    private fun readRoot(): JSONObject? {
        if (!settingsFile.isFile) return null
        return JSONObject(settingsFile.readText(StandardCharsets.UTF_8))
    }
}

/** Import only the prior LingXi app-private files into the same LingXi profile. */
internal fun permissionModeLegacySettingsDirectory(
    filesDir: File,
    brandId: String,
    linuxUser: String,
    configDirName: String,
): File? = if (brandId == "lingxi" && linuxUser == "lingxi" && configDirName == ".lingxi") {
    File(filesDir, ".lingxi")
} else {
    null
}

/** Copy the two owned files once; keep the source as a recoverable user-data copy. */
internal fun copyPermissionSettingsIfMissing(sourceDirectory: File, targetDirectory: File) {
    val files = listOf("settings.json", "last-permission-mode.json")
    for (name in files) {
        val source = File(sourceDirectory, name)
        val target = File(targetDirectory, name)
        if (!source.isFile || target.exists()) continue
        targetDirectory.mkdirs()
        val temporary = Files.createTempFile(targetDirectory.toPath(), ".$name.", ".profile-import")
        try {
            Files.copy(source.toPath(), temporary, StandardCopyOption.REPLACE_EXISTING)
            try {
                // No REPLACE_EXISTING: preserve a target created concurrently by the engine.
                // The temp and destination share a directory, so the move stays on one filesystem.
                Files.move(temporary, target.toPath())
            } catch (_: FileAlreadyExistsException) {
                // Another engine or repository writer initialized the target.
            }
        } finally {
            Files.deleteIfExists(temporary)
        }
    }
}

/** Pure JSON seam kept separate so unknown-field preservation is JVM-testable. */
internal object PermissionModeSettingsJson {
    fun load(raw: String?, lastSelection: String? = null): String {
        val selected = runCatching {
            lastSelection?.let { JSONObject(it).optString("mode") }
        }.getOrNull()
        if (selected != null && selected in PermissionModeOptions.values) return selected
        val root = try {
            raw?.let { JSONObject(it) }
        } catch (_: Exception) {
            return "auto"
        } ?: return "auto"
        return root.optJSONObject("permissions")?.optString("defaultMode")
            ?.takeIf { it in PermissionModeOptions.values }
            ?: "auto"
    }

    fun update(raw: String?, mode: String): String {
        require(mode in PermissionModeOptions.values) { "unknown permission mode: $mode" }
        val root = raw?.let { JSONObject(it) } ?: JSONObject()
        val permissions = root.optJSONObject("permissions") ?: JSONObject()
        permissions.put("defaultMode", mode)
        root.put("permissions", permissions)
        return root.toString()
    }
}

object PermissionModeOptions {
    val values: Set<String> = linkedSetOf(
        "default",
        "acceptEdits",
        "plan",
        "auto",
        "dontAsk",
        "bypassPermissions",
    )
}
