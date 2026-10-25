package com.lingxi.code.settings

import android.content.Context
import android.os.Build
import com.lingxi.code.bindings.android.AndroidMobileLinuxConfigFfi
import com.lingxi.code.bindings.android.AndroidMobileLinuxEventSink
import com.lingxi.code.bindings.android.AndroidMobileLinuxRootfsStager
import com.lingxi.code.bindings.android.AndroidMobileLinuxRuntimeHandle
import com.lingxi.code.bindings.android.MobileLinuxCommandRequestFfi
import com.lingxi.code.bindings.android.MobileLinuxCommandResultFfi
import com.lingxi.code.bindings.android.MobileLinuxEventFfi
import com.lingxi.code.bindings.android.MobileLinuxMountSpecFfi
import com.lingxi.code.bindings.android.MobileLinuxProcessHandleFfi
import com.lingxi.code.bindings.android.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySizeFfi
import com.lingxi.code.bindings.android.MobileLinuxRootfsStateFfi
import com.lingxi.code.bindings.android.MobileLinuxStatusFfi
import com.lingxi.code.bindings.android.MobileLinuxTaskSnapshotFfi
import com.lingxi.code.bindings.android.buildAndroidMobileLinuxRuntimeHandle
import com.lingxi.code.project.ProjectWorkspace
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.nio.charset.StandardCharsets
import java.util.UUID
import org.json.JSONObject

object LinuxRuntimeBridge {
    @Volatile
    private var cachedRuntime: Pair<String, AndroidMobileLinuxRuntimeHandle>? = null
    private fun workspaceGuestPath(context: Context): String =
        com.lingxi.code.project.MobileUserStorage.directories(context).defaultGuestCwd
    // Packaged assets are immutable for the APK lifetime; event polling must
    // not parse the full rootfs inventory on every request.
    @Volatile
    private var cachedBundledIdentity: Pair<String, RootfsArtifactIdentity>? = null

    fun managedRoot(context: Context): String =
        File(context.applicationContext.filesDir, "mobile-linux/android-proot").absolutePath

    private fun appSandboxRoot(context: Context): String =
        context.applicationContext.filesDir.canonicalFile.absolutePath


    private fun abi(): String = Build.SUPPORTED_ABIS.firstOrNull() ?: "unknown"

    fun configForWorkspace(
        context: Context,
        mode: LinuxRuntimeMode,
        workspace: ProjectWorkspace? = null,
    ): AndroidMobileLinuxConfigFfi {
        val paths = com.lingxi.code.project.MobileUserStorage.directories(context)
        val hostPath = paths.projects
        val hostDirectory = File(hostPath)
        check(hostDirectory.isDirectory || hostDirectory.mkdirs()) {
            "Android PRoot workspace is unavailable: $hostPath"
        }
        val selectedAbi = abi()
        val identity = bundledIdentity(context, selectedAbi)
        return mobileLinuxConfig(
            brandId = paths.brandId,
            linuxUser = paths.linuxUser,
            configDirName = paths.configDirName,
            managedRoot = managedRoot(context),
            appSandboxRoot = appSandboxRoot(context),
            workspaceHostPath = hostPath,
            stableWorkspaceId = "projects",
            abi = selectedAbi,
            rootfsIdentity = identity,
            mode = mode,
        )
    }

    private fun bundledIdentity(context: Context, abi: String): RootfsArtifactIdentity {
        val application = context.applicationContext
        val cacheKey = "${application.packageName}|${application.applicationInfo.sourceDir}|$abi"
        cachedBundledIdentity?.takeIf { it.first == cacheKey }?.let {
            return it.second
        }
        val assets = application.assets
        val pinsName = "mobile-linux-pins.json"
        if (assets.list("mobile-linux")?.contains(pinsName) != true) {
            error("Mobile Linux release assets are missing from this application")
        }
        val pinsJson = assets.open("mobile-linux/$pinsName").bufferedReader(StandardCharsets.UTF_8).use { it.readText() }
        val pins = JSONObject(pinsJson)
        val releases = pins.getJSONObject("rootfs").optJSONObject("release_archives")
        if (releases?.has(abi) != true) {
            error("No bundled release rootfs for ABI $abi")
        }
        val assetDir = "mobile-linux/rootfs/$abi"
        val manifestJson = assets.open("$assetDir/$ROOTFS_MANIFEST_FILE")
            .bufferedReader(StandardCharsets.UTF_8).use { it.readText() }
        val identity = bundledRootfsIdentity(pinsJson, manifestJson, abi)
        check(assets.list(assetDir)?.contains("${identity.filename}.bin") == true) {
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
        ).joinToString("|")
        cachedRuntime?.takeIf { it.first == key }?.let { return it.second }
        return buildAndroidMobileLinuxRuntimeHandle(config).also { cachedRuntime = key to it }
    }

    private fun mobileRuntime(
        context: Context,
        mode: LinuxRuntimeMode,
    ): AndroidMobileLinuxRuntimeHandle {
        return runtime(config(context, mode))
    }

    suspend fun load(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            val capability = runtime.capability()
            val status = runtime.status()
            linuxRuntimeUiStateFrom(
                mode = mode,
                capability = capability,
                status = status,
                workspaceGuestPath = workspaceGuestPath(context),
                lastAction = LinuxRuntimeAction.Refresh,
                tasks = runtime.listTasks(),
            )
        }

    /** Advisory UI check; FFI repeats the check under its maintenance lease. */
    private suspend fun requireMaintenanceIdle(context: Context, runtime: AndroidMobileLinuxRuntimeHandle) {
        val apps = com.lingxi.code.apps.UnifiedLocalApps.get(context)
        val records = apps.request("list") as org.json.JSONArray
        val blockers = mutableListOf<String>()
        for (index in 0 until records.length()) {
            val record = records.getJSONObject(index)
            val appId = record.getString("appId")
            for (run in apps.runs(appId)) {
                if (run.phase in setOf("preparing", "starting", "running", "stopping") && apps.release(appId, run.revision).executesCode) {
                    blockers.add("${record.getString("name")} · appId=$appId · runId=${run.runId} · ${run.phase}")
                }
            }
        }
        val activeStates = setOf(
            com.lingxi.code.bindings.android.MobileLinuxTaskStateFfi.QUEUED,
            com.lingxi.code.bindings.android.MobileLinuxTaskStateFfi.RUNNING,
            com.lingxi.code.bindings.android.MobileLinuxTaskStateFfi.BACKGROUNDED,
        )
        for (task in runtime.listTasks().filter { it.status in activeStates }) {
            blockers.add("SDK taskId=${task.taskId} · ${task.command} · ${task.status}")
        }
        check(blockers.isEmpty()) {
            context.getString(com.lingxi.code.R.string.local_app_v2_maintenance_blocked) + "\n\n" + blockers.joinToString("\n")
        }
    }

    suspend fun verify(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            val capability = runtime.capability()
            val status = runtime.verifyRootfs()
            linuxRuntimeUiStateFrom(mode, capability, status, workspaceGuestPath(context), LinuxRuntimeAction.Verify)
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
            requireMaintenanceIdle(context, runtime)
            val status = runtime.stageAndRepairRootfs(object : AndroidMobileLinuxRootfsStager {
                override suspend fun stage() = withContext(Dispatchers.IO) {
                    stageBundledRootfs(context.applicationContext, config(context, mode))
                }
            })
            val capability = runtime.capability()
            linuxRuntimeUiStateFrom(mode, capability, status, workspaceGuestPath(context), LinuxRuntimeAction.Install)
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
        val archiveName = bundledIdentity(context, config.abi).filename
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
                context.assets.open("$assetDir/$archiveName.bin").use { input ->
                    FileOutputStream(archive).use(input::copyTo)
                }
            },
        )
    }

    suspend fun repair(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            val capability = runtime.capability()
            requireMaintenanceIdle(context, runtime)
            val status = runtime.stageAndRepairRootfs(object : AndroidMobileLinuxRootfsStager {
                override suspend fun stage() = withContext(Dispatchers.IO) {
                    // Recreate the pinned manifest too: status can report a
                    // runnable shell even when that manifest is missing.
                    stageBundledRootfs(context.applicationContext, config)
                }
            })
            linuxRuntimeUiStateFrom(mode, capability, status, workspaceGuestPath(context), LinuxRuntimeAction.Repair)
        }

    suspend fun reset(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            val capability = runtime.capability()
            requireMaintenanceIdle(context, runtime)
            val status = runtime.resetRootfs()
            linuxRuntimeUiStateFrom(mode, capability, status, workspaceGuestPath(context), LinuxRuntimeAction.Reset)
        }

    suspend fun boot(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            val capability = runtime.capability()
            val status = runtime.boot()
            linuxRuntimeUiStateFrom(mode, capability, status, workspaceGuestPath(context), LinuxRuntimeAction.Boot)
        }

    suspend fun shutdown(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            requireMaintenanceIdle(context, runtime)
            runtime.shutdown()
            val capability = runtime.capability()
            val status = runtime.status()
            linuxRuntimeUiStateFrom(mode, capability, status, workspaceGuestPath(context), LinuxRuntimeAction.Shutdown)
        }

    suspend fun refreshTasks(context: Context, mode: LinuxRuntimeMode): LinuxRuntimeUiState =
        withContext(Dispatchers.IO) {
            val config = config(context, mode)
            val runtime = runtime(config)
            val capability = runtime.capability()
            val status = runtime.status()
            linuxRuntimeUiStateFrom(
                mode = mode,
                capability = capability,
                status = status,
                workspaceGuestPath = workspaceGuestPath(context),
                lastAction = LinuxRuntimeAction.RefreshTasks,
                tasks = runtime.listTasks(),
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
        val runtime = mobileRuntime(context, mode)
        requireMaintenanceIdle(context, runtime)
        runtime.configureMounts(mounts)
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
