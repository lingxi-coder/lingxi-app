package com.lingxi.code.settings

import android.content.Context
import android.os.Build
import com.lingxi.code.bindings.AndroidMobileLinuxConfigFfi
import com.lingxi.code.bindings.AndroidMobileLinuxEventSink
import com.lingxi.code.bindings.AndroidMobileLinuxRuntimeHandle
import com.lingxi.code.bindings.MobileLinuxCommandRequestFfi
import com.lingxi.code.bindings.MobileLinuxCommandResultFfi
import com.lingxi.code.bindings.MobileLinuxEventFfi
import com.lingxi.code.bindings.MobileLinuxMountSpecFfi
import com.lingxi.code.bindings.MobileLinuxProcessHandleFfi
import com.lingxi.code.bindings.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.MobileLinuxPtySizeFfi
import com.lingxi.code.bindings.MobileLinuxRootfsStateFfi
import com.lingxi.code.bindings.MobileLinuxStatusFfi
import com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi
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
import java.io.FileOutputStream
import java.nio.charset.StandardCharsets
import java.util.UUID
import org.json.JSONObject

object LinuxRuntimeBridge {
    private const val WORKSPACE_PREFERENCES = "lingxi_mobile_linux"
    private const val DEFAULT_WORKSPACE_ID_KEY = "default_workspace_id"
    @Volatile
    private var cachedRuntime: Pair<String, AndroidMobileLinuxRuntimeHandle>? = null
    // Packaged assets are immutable for the APK lifetime; event polling must
    // not parse the full rootfs inventory on every request.
    @Volatile
    private var cachedBundledIdentity: Pair<String, RootfsArtifactIdentity?>? = null

    private fun managedRoot(context: Context): String =
        File(context.applicationContext.filesDir, "mobile-linux/android-proot").absolutePath

    private fun appSandboxRoot(context: Context): String =
        context.applicationContext.filesDir.canonicalFile.absolutePath

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
        val selectedAbi = abi()
        val identity = bundledIdentity(context, selectedAbi, required = mode == LinuxRuntimeMode.MobileLinux)
        return mobileLinuxConfig(
            managedRoot = managedRoot(context),
            appSandboxRoot = appSandboxRoot(context),
            workspaceHostPath = hostPath,
            stableWorkspaceId = workspaceID,
            abi = selectedAbi,
            rootfsIdentity = identity,
            mode = mode,
            authorizationFile = authorizationFile(context),
        )
    }

    private fun bundledIdentity(context: Context, abi: String, required: Boolean): RootfsArtifactIdentity? {
        val application = context.applicationContext
        val cacheKey = "${application.packageName}|${application.applicationInfo.sourceDir}|$abi"
        cachedBundledIdentity?.takeIf { it.first == cacheKey }?.let {
            check(!required || it.second != null) { "No bundled release rootfs for ABI $abi" }
            return it.second
        }
        val assets = application.assets
        val pinsName = "mobile-linux-pins.json"
        if (assets.list("mobile-linux")?.contains(pinsName) != true) {
            check(!required) { "Mobile Linux release assets are missing from this application" }
            cachedBundledIdentity = cacheKey to null
            return null
        }
        val pinsJson = assets.open("mobile-linux/$pinsName").bufferedReader(StandardCharsets.UTF_8).use { it.readText() }
        val pins = JSONObject(pinsJson)
        val releases = pins.getJSONObject("rootfs").optJSONObject("release_archives")
        if (releases?.has(abi) != true) {
            check(!required) { "No bundled release rootfs for ABI $abi" }
            cachedBundledIdentity = cacheKey to null
            return null
        }
        val assetDir = "mobile-linux/rootfs/$abi"
        val manifestJson = assets.open("$assetDir/$ROOTFS_MANIFEST_FILE")
            .bufferedReader(StandardCharsets.UTF_8).use { it.readText() }
        val identity = bundledRootfsIdentity(pinsJson, manifestJson, abi)
        check(assets.list(assetDir)?.contains(identity.filename) == true) {
            "Bundled rootfs archive ${identity.filename} is missing for ABI $abi"
        }
        cachedBundledIdentity = cacheKey to identity
        return identity
    }

    private fun config(context: Context, mode: LinuxRuntimeMode): AndroidMobileLinuxConfigFfi =
        configForWorkspace(context, mode)

    @Synchronized
    private fun runtime(config: AndroidMobileLinuxConfigFfi): AndroidMobileLinuxRuntimeHandle {
        val key = listOf(
            config.mode,
            config.managedRoot,
            config.appSandboxRoot,
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

    private fun mobileRuntime(
        context: Context,
        mode: LinuxRuntimeMode,
    ): AndroidMobileLinuxRuntimeHandle {
        check(mode == LinuxRuntimeMode.MobileLinux) {
            "Mobile Linux operation rejected because the Legacy runtime is selected"
        }
        return runtime(config(context, mode))
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

    /**
     * Activates a host-staged rootfs through the generated repair endpoint.
     *
     * The shared runtime has no parallel install method or archive payload.
     * Repair validates and atomically activates staged content; if staging is
     * absent or invalid, the returned status remains missing/corrupt.
     */
    suspend fun install(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val runtime = mobileRuntime(context, mode)
            val before = runtime.status()
            check(before.state == MobileLinuxRootfsStateFfi.MISSING) {
                "Rootfs installation requires MISSING state; current state is ${before.state}"
            }
            stageBundledRootfs(context.applicationContext, config(context, mode))
            val status = runtime.repairRootfs()
            val capability = runtime.capability()
            linuxRuntimeUiStateFrom(mode, capability, status, LinuxRuntimeAction.Install)
        }

    /**
     * Verify and unpack the ABI-specific, build-pinned Alpine archive into a
     * fresh staging directory. Activation remains owned by the Rust runtime,
     * which renames staged -> active atomically.
     */
    private fun stageBundledRootfs(
        context: Context,
        config: AndroidMobileLinuxConfigFfi,
    ) {
        val expectedSha = checkNotNull(config.archiveSha256) {
            "No pinned rootfs archive exists for ABI ${config.abi}"
        }
        val assetDir = "mobile-linux/rootfs/${config.abi}"
        val archiveName = context.assets.list(assetDir)
            ?.singleOrNull { it.endsWith(".tar.gz") }
            ?: error("Bundled rootfs archive is missing for ${config.abi}")
        BundledRootfsInstaller.stage(
            managedRoot = File(config.managedRoot).canonicalFile,
            expectedRootfsVersion = config.rootfsVersion,
            expectedArchiveSha = expectedSha,
            expectedManifestAbi = manifestAbiFor(config.abi),
            archiveName = archiveName,
            manifestJson = context.assets
                .open("$assetDir/$ROOTFS_MANIFEST_FILE")
                .bufferedReader(StandardCharsets.UTF_8)
                .use { it.readText() },
            sbomJson = context.assets
                .open("$assetDir/$ROOTFS_SBOM_FILE")
                .bufferedReader(StandardCharsets.UTF_8)
                .use { it.readText() },
            copyArchive = { archive ->
                context.assets.open("$assetDir/$archiveName").use { input ->
                    FileOutputStream(archive).use(input::copyTo)
                }
            },
        )
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

    suspend fun runCommandStreaming(
        context: Context,
        mode: LinuxRuntimeMode,
        request: MobileLinuxCommandRequestFfi,
        sink: AndroidMobileLinuxEventSink,
    ): MobileLinuxCommandResultFfi = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).runCommandStreaming(request, sink)
    }

    suspend fun spawnBackground(
        context: Context,
        mode: LinuxRuntimeMode,
        request: MobileLinuxCommandRequestFfi,
    ): MobileLinuxProcessHandleFfi = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).spawnBackground(request)
    }

    suspend fun killProcess(
        context: Context,
        mode: LinuxRuntimeMode,
        handle: MobileLinuxProcessHandleFfi,
    ) = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).killProcess(handle)
    }

    suspend fun openPty(
        context: Context,
        mode: LinuxRuntimeMode,
        request: MobileLinuxPtyOpenRequestFfi,
    ): MobileLinuxPtySessionHandleFfi = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).openPty(request)
    }

    suspend fun writePty(
        context: Context,
        mode: LinuxRuntimeMode,
        handle: MobileLinuxPtySessionHandleFfi,
        input: ByteArray,
    ) = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).writePty(handle, input)
    }

    suspend fun resizePty(
        context: Context,
        mode: LinuxRuntimeMode,
        handle: MobileLinuxPtySessionHandleFfi,
        size: MobileLinuxPtySizeFfi,
    ) = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).resizePty(handle, size)
    }

    suspend fun closePty(
        context: Context,
        mode: LinuxRuntimeMode,
        handle: MobileLinuxPtySessionHandleFfi,
    ) = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).closePty(handle)
    }

    suspend fun configureMounts(
        context: Context,
        mode: LinuxRuntimeMode,
        mounts: List<MobileLinuxMountSpecFfi>,
    ): MobileLinuxStatusFfi = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).configureMounts(mounts)
    }

    suspend fun readEvents(
        context: Context,
        mode: LinuxRuntimeMode,
        afterSequence: ULong? = null,
        limit: UInt? = null,
    ): List<MobileLinuxEventFfi> = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).readEvents(afterSequence, limit)
    }

    suspend fun listTasks(
        context: Context,
        mode: LinuxRuntimeMode,
    ): List<MobileLinuxTaskSnapshotFfi> = withContext(Dispatchers.IO) {
        mobileRuntime(context, mode).listTasks()
    }

    suspend fun taskStatus(
        context: Context,
        mode: LinuxRuntimeMode,
        taskId: String,
    ): MobileLinuxTaskSnapshotFfi? = withContext(Dispatchers.IO) {
        require(taskId.isNotBlank()) { "taskId must not be blank" }
        mobileRuntime(context, mode).taskStatus(taskId)
    }
}

private const val ROOTFS_MANIFEST_FILE = "rootfs-manifest.json"
private const val ROOTFS_SBOM_FILE = "rootfs.spdx.json"

private fun manifestAbiFor(abi: String): String =
    when (abi) {
        "arm64-v8a" -> "arm64"
        "x86_64" -> "x86_64"
        else -> error("Unsupported rootfs ABI for manifest validation: $abi")
    }
