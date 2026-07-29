package com.lingxi.code.settings

import android.content.Context
import android.os.Build
import com.lingxi.code.bindings.AndroidMobileLinuxConfigFfi
import com.lingxi.code.bindings.AndroidMobileLinuxRuntimeHandle
import com.lingxi.code.bindings.androidMobileLinuxCapability
import com.lingxi.code.bindings.androidMobileLinuxBoot
import com.lingxi.code.bindings.androidMobileLinuxListTasks
import com.lingxi.code.bindings.androidMobileLinuxRepairRootfs
import com.lingxi.code.bindings.androidMobileLinuxResetRootfs
import com.lingxi.code.bindings.androidMobileLinuxShutdown
import com.lingxi.code.bindings.androidMobileLinuxStatus
import com.lingxi.code.bindings.androidMobileLinuxVerifyRootfs
import com.lingxi.code.bindings.buildAndroidMobileLinuxRuntimeHandle
import com.lingxi.code.project.ProjectWorkspace
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.util.UUID

object LinuxRuntimeBridge {
    private const val WORKSPACE_PREFERENCES = "lingxi_mobile_linux"
    private const val DEFAULT_WORKSPACE_ID_KEY = "default_workspace_id"
    @Volatile
    private var cachedRuntime: Pair<String, AndroidMobileLinuxRuntimeHandle>? = null

    private fun managedRoot(context: Context): String =
        File(context.applicationContext.filesDir, "mobile-linux/android-proot").absolutePath

    private fun workspaceHostPath(context: Context, workspaceID: String): String =
        File(context.applicationContext.filesDir, "workspaces/$workspaceID")
            .also { it.mkdirs() }
            .absolutePath

    private fun stableWorkspaceId(context: Context): String {
        val preferences = context.applicationContext.getSharedPreferences(
            WORKSPACE_PREFERENCES,
            Context.MODE_PRIVATE,
        )
        val persisted = preferences.getString(DEFAULT_WORKSPACE_ID_KEY, null)
        if (persisted != null && runCatching { UUID.fromString(persisted) }.isSuccess) {
            return persisted.lowercase()
        }
        val generated = UUID.randomUUID().toString().lowercase()
        preferences.edit().putString(DEFAULT_WORKSPACE_ID_KEY, generated).apply()
        return generated
    }

    private fun authorizationFile(context: Context): String =
        File(
            context.applicationContext.filesDir,
            "mobile-linux/authorization/AUTHORIZATION_MANIFEST.json",
        ).absolutePath

    private fun abi(): String = Build.SUPPORTED_ABIS.firstOrNull() ?: "unknown"

    fun configForWorkspace(
        context: Context,
        mode: LinuxRuntimeMode,
        workspace: ProjectWorkspace? = null,
    ): AndroidMobileLinuxConfigFfi {
        val workspaceID = workspace?.projectId ?: stableWorkspaceId(context)
        val hostPath = workspace?.hostPath ?: workspaceHostPath(context, workspaceID)
        return mobileLinuxConfig(
            managedRoot = managedRoot(context),
            workspaceHostPath = hostPath,
            stableWorkspaceId = workspaceID,
            abi = abi(),
            mode = mode,
            authorizationFile = authorizationFile(context),
        )
    }

    private fun config(context: Context, mode: LinuxRuntimeMode): AndroidMobileLinuxConfigFfi =
        configForWorkspace(context, mode)

    @Synchronized
    private fun runtime(config: AndroidMobileLinuxConfigFfi): AndroidMobileLinuxRuntimeHandle {
        val key = listOf(
            config.mode,
            config.managedRoot,
            config.workspaceHostPath,
            config.stableWorkspaceId,
            config.abi,
            config.rootfsVersion,
            config.archiveSha256,
            config.authorizationFile,
        ).joinToString("|")
        cachedRuntime?.takeIf { it.first == key }?.let { return it.second }
        return buildAndroidMobileLinuxRuntimeHandle(config).also { cachedRuntime = key to it }
    }

    suspend fun load(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val persistent = mode == LinuxRuntimeMode.MobileLinux
            val runtime = if (persistent) runtime(config) else null
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.status() ?: androidMobileLinuxStatus(config)
            linuxRuntimeUiStateFrom(
                mode = mode,
                capability = capability,
                status = status,
                lastAction = LinuxRuntimeAction.Refresh,
                tasks = runCatching {
                    runtime?.listTasks() ?: androidMobileLinuxListTasks(config)
                }.getOrDefault(emptyList()),
            )
        }

    suspend fun verify(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = if (mode == LinuxRuntimeMode.MobileLinux) runtime(config) else null
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.verifyRootfs() ?: androidMobileLinuxVerifyRootfs(config)
            linuxRuntimeUiStateFrom(mode, capability, status, LinuxRuntimeAction.Verify)
        }

    suspend fun repair(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = if (mode == LinuxRuntimeMode.MobileLinux) runtime(config) else null
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.repairRootfs() ?: androidMobileLinuxRepairRootfs(config)
            linuxRuntimeUiStateFrom(mode, capability, status, LinuxRuntimeAction.Repair)
        }

    suspend fun reset(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = if (mode == LinuxRuntimeMode.MobileLinux) runtime(config) else null
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.resetRootfs() ?: androidMobileLinuxResetRootfs(config)
            linuxRuntimeUiStateFrom(mode, capability, status, LinuxRuntimeAction.Reset)
        }

    suspend fun boot(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = if (mode == LinuxRuntimeMode.MobileLinux) runtime(config) else null
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.boot() ?: androidMobileLinuxBoot(config)
            linuxRuntimeUiStateFrom(mode, capability, status, LinuxRuntimeAction.Boot)
        }

    suspend fun shutdown(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = if (mode == LinuxRuntimeMode.MobileLinux) runtime(config) else null
            if (runtime != null) runtime.shutdown() else androidMobileLinuxShutdown(config)
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.status() ?: androidMobileLinuxStatus(config)
            linuxRuntimeUiStateFrom(mode, capability, status, LinuxRuntimeAction.Shutdown)
        }

    suspend fun refreshTasks(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = if (mode == LinuxRuntimeMode.MobileLinux) runtime(config) else null
            val capability = runtime?.capability() ?: androidMobileLinuxCapability(config)
            val status = runtime?.status() ?: androidMobileLinuxStatus(config)
            linuxRuntimeUiStateFrom(
                mode = mode,
                capability = capability,
                status = status,
                lastAction = LinuxRuntimeAction.RefreshTasks,
                tasks = runtime?.listTasks() ?: androidMobileLinuxListTasks(config),
            )
        }
}
