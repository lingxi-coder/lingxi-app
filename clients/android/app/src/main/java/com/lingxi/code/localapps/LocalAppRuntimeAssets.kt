package com.lingxi.code.localapps

import android.content.Context
import com.lingxi.code.R
import java.io.File
import java.io.FileInputStream
import java.nio.file.Files
import java.nio.file.LinkOption
import java.security.MessageDigest
import java.util.concurrent.Callable
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import org.json.JSONArray
import org.json.JSONObject

/**
 * Tracks staging of the bundled local-app runtime into app-private storage.
 *
 * The runtime extraction is large enough to miss main-thread startup budgets,
 * so callers need two properties at once:
 * 1. never block on a cold extract;
 * 2. never hand the engine a `null` root when the APK really does bundle a
 *    runtime tree.
 *
 * The contract here is therefore "asset presence promises a destination path".
 * As soon as the APK asset root is confirmed, callers get a manifest-addressed
 * destination path in `filesDir/local-app-runtime/<manifest-sha256>` and a
 * single background extraction starts filling it atomically. Callers that need
 * the files themselves can wait for the promised path to become ready; callers
 * that only need configuration no longer race on a transient `null`, and old
 * runtime versions cannot satisfy readiness for a new manifest.
 */
internal enum class LocalAppRuntimeStaging {
    /** Nothing has asked for the runtime yet in this process. */
    Idle,

    /** A staging run is extracting the tree right now. */
    Staging,

    /** The runtime tree is present and passes the readiness check. */
    Ready,

    /** A staging run finished and produced no usable runtime. */
    Unavailable,
}

/** Stages the build-verified runtime asset into the app-private filesystem. */
object LocalAppRuntimeAssets {
    private const val ASSET_ROOT = "local-app-runtime"
    private const val REQUIRED_VITE_PATH = "node_modules/vite/bin/vite.js"
    private const val REQUIRED_PLATFORM = "android"
    private const val FAILURE_MARKER_SUFFIX = ".failed"
    private const val READY_MARKER_SUFFIX = ".ready"
    private const val FAILURE_MARKER_MAX_CHARS = 512
    private const val READY_MARKER_MAX_CHARS = 64
    private const val RUNTIME_MANIFEST_PATH = "runtime-manifest.json"
    private val REQUIRED_INVENTORY_PATHS =
        setOf(
            REQUIRED_VITE_PATH,
            "runtime-policy.json",
            "runtime-pins.json",
            "runtime.spdx.json",
        )

    /**
     * Every caller reaches this from `buildVoiceEngine`, which has THREE
     * production call sites — `conversation/ConversationSource.kt`,
     * `settings/ProviderSettingsRepository.kt` and `cron/HeadlessEngineFactory.kt`
     * — and the budget is sized for the worst of them: `ConversationSource`'s
     * `create` is invoked from `RootScreen`'s `viewModel { initializer { … } }`
     * block, which Compose runs synchronously on the MAIN thread. Extracting the
     * ~200 MB runtime takes tens of seconds there, well past the 5 s
     * input-dispatch ANR, so the work runs on a background thread and the caller
     * waits only this long.
     *
     * Unlike the old behaviour, a budget overrun no longer returns `null` if
     * the APK really does bundle the runtime. The caller gets the stable
     * destination path immediately, while the background extraction keeps
     * running. Widening this budget would only burn ANR margin without changing
     * correctness, so it stays where the UI thread can afford it.
     */
    private const val STAGE_BUDGET_MS = 1_500L

    @Volatile
    private var stagedRoot: String? = null

    private val lock = Any()
    private var inFlight: FutureTask<String?>? = null

    /**
     * Set when a run finishes with no root. Never cleared on its own: a LIVE
     * run outranks it in [stagingStatus], and a run that succeeds sets
     * [stagedRoot], which outranks both.
     */
    @Volatile
    private var lastRunProducedNothing = false

    fun prepare(context: Context): String? {
        val appContext = context.applicationContext
        return prepareWithinPlan(
            budgetMs = STAGE_BUDGET_MS,
            planProvider = { runtimePlan(appContext) },
            stage = { plan -> stage(appContext, plan) },
        )
    }

    /**
     * Where staging got to, from the perspective of a surface that has to
     * explain a failure. Cheap and non-blocking: it only ever calls `get()` on
     * a task that has already completed, so it is safe from a reducer on the
     * main thread.
     */
    internal fun stagingStatus(): LocalAppRuntimeStaging {
        synchronized(lock) { inFlight }?.let(::harvest)
        return when {
            stagedRoot != null -> LocalAppRuntimeStaging.Ready
            synchronized(lock) { inFlight } != null -> LocalAppRuntimeStaging.Staging
            lastRunProducedNothing -> LocalAppRuntimeStaging.Unavailable
            else -> LocalAppRuntimeStaging.Idle
        }
    }

    /**
     * The `detail` a generation job should carry. A job that FAILED while this
     * process has no usable runtime root gets the reason appended, because the
     * engine's own message ("stage local-app-runtime first") describes a build
     * step, not anything the user can do on the device.
     *
     * Appends rather than replaces: whatever the engine said stays first, since
     * the runtime is not necessarily the only thing that went wrong.
     *
     * [LocalAppRuntimeStaging.Staging] and [LocalAppRuntimeStaging.Unavailable]
     * annotate EVERY failure. While either status is readable there is still no
     * ready runtime tree at the promised path, so retrying a build immediately
     * is either premature or guaranteed to fail the same way.
     */
    internal fun generationDetail(
        detail: String?,
        failed: Boolean,
        strings: LocalAppsStrings = DefaultLocalAppsStrings,
    ): String? {
        if (!failed) return detail
        val notice = noticeFor(stagingStatus(), strings) ?: return detail
        return listOfNotNull(detail?.takeIf(String::isNotBlank), notice).joinToString("\n\n")
    }

    internal fun noticeFor(
        status: LocalAppRuntimeStaging,
        strings: LocalAppsStrings = DefaultLocalAppsStrings,
    ): String? = when (status) {
        LocalAppRuntimeStaging.Ready, LocalAppRuntimeStaging.Idle -> null
        LocalAppRuntimeStaging.Staging ->
            strings.resolve(
                R.string.local_apps_runtime_notice_staging,
                "本地应用运行时仍在首次准备中。请等待准备完成后直接重试生成应用。",
            )
        LocalAppRuntimeStaging.Unavailable ->
            strings.resolve(
                R.string.local_apps_runtime_notice_unavailable,
                "本地应用运行时准备失败（安装包中没有运行时，或解压失败），本次启动无法生成应用。",
            )
    }

    /**
     * Single-flight staging with a bounded wait. A staging run that outlives
     * the budget keeps going: a later call joins the same task and observes its
     * result, so the tree is never staged twice concurrently. A run that yields
     * `null` is not memoised, so a transient IO error retries on the next
     * engine build.
     */
    internal fun prepareWithin(
        budgetMs: Long,
        promisedRoot: String? = null,
        stage: () -> String?,
    ): String? {
        stagedRoot?.let { return it }
        val task: FutureTask<String?> = synchronized(lock) {
            stagedRoot?.let { return it }
            inFlight ?: FutureTask(Callable { stage() }).also {
                inFlight = it
                Thread(it, "local-app-runtime-stage").apply { isDaemon = true }.start()
            }
        }
        val staged: String? = try {
            task.get(budgetMs, TimeUnit.MILLISECONDS)
        } catch (timeout: TimeoutException) {
            null
        } catch (interrupted: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        }
        harvest(task)
        return staged ?: promisedRoot
    }

    internal fun prepareWithinPlan(
        budgetMs: Long,
        planProvider: () -> RuntimePlan?,
        stage: (RuntimePlan?) -> String?,
    ): String? {
        stagedRoot?.let { return it }
        val plan = runCatching(planProvider).getOrNull()
        return prepareWithin(budgetMs, plan?.destination?.absolutePath) { stage(plan) }
    }

    /**
     * Retire a finished run: memoise a root, or remember that the run produced
     * none. A no-op while the run is still going, so the "not memoised on
     * failure, so a transient IO error retries" property above is unchanged.
     */
    private fun harvest(task: FutureTask<String?>) {
        if (!task.isDone) return
        // A throwing stage cannot happen on the production path (`stage` is
        // runCatching-wrapped), but this also runs from `stagingStatus` inside
        // a reducer, where an ExecutionException must not escape.
        val result = runCatching { task.get() }.getOrNull()
        synchronized(lock) { if (inFlight === task) inFlight = null }
        if (result != null) stagedRoot = result else lastRunProducedNothing = true
    }

    /** Drops every memo so each test observes a cold process. */
    internal fun resetForTests() {
        synchronized(lock) {
            inFlight = null
            stagedRoot = null
            lastRunProducedNothing = false
        }
    }

    /**
     * Staging an OPTIONAL runtime must never take the engine with it: every
     * caller sits inside `buildVoiceEngine`, whose catch-all turns a throw here
     * into "no engine at all" — chat, sessions and cron included. A failure
     * means local apps are unavailable, and it is not memoised, so a transient
     * IO error retries on the next engine build.
     */
    private fun stage(context: Context, plan: RuntimePlan?): String? = runCatching {
        val livePlan = plan ?: runtimePlan(context) ?: return@runCatching null
        val destination = livePlan.destination
        deleteFailureMarker(destination)
        if (!runtimeIsReady(destination, livePlan.expectedManifest)) {
            if (recoverReadyMarkerIfInventoryStillMatches(destination, livePlan.expectedManifest)) {
                return@runCatching destination.absolutePath
            }
            deleteReadyMarker(destination)
            val container = destination.parentFile ?: return@runCatching null
            container.mkdirs()
            // filesDir, not cacheDir: the rename is then a same-mount atomic
            // move, so a copy that dies mid-tree can never leave a destination
            // whose manifest matches a partially staged runtime.
            val staging = File(container, "${destination.name}.staging")
            staging.deleteRecursively()
            staging.mkdirs()
            copyAssetTree(context, ASSET_ROOT, staging)
            sealRegularFilesReadOnly(staging)
            if (!runtimeInventoryIsValid(staging, livePlan.expectedManifest)) {
                publishFailureMarker(
                    destination,
                    "runtime seed inventory validation failed before publish",
                )
                return@runCatching null
            }
            if (!replaceAtomically(staging, destination)) {
                publishFailureMarker(
                    destination,
                    "runtime seed could not be promoted atomically",
                )
                return@runCatching null
            }
            if (!publishReadyMarker(destination)) {
                publishFailureMarker(
                    destination,
                    "runtime seed could not publish a validated readiness marker",
                )
                return@runCatching null
            }
            cleanupObsoleteDigests(container, keep = destination.name)
        }
        destination
            .takeIf { runtimeIsReady(it, livePlan.expectedManifest) }
            ?.absolutePath
            ?.also { deleteFailureMarker(destination) }
    }.getOrElse { error ->
        plan?.destination?.let { destination ->
            publishFailureMarker(
                destination,
                "runtime seed staging failed: ${error.message ?: error::class.java.simpleName}",
            )
        }
        null
    }

    private fun runtimePlan(context: Context): RuntimePlan? {
        val assetEntries = context.assets.list(ASSET_ROOT).orEmpty()
        if (assetEntries.isEmpty()) return null
        val expectedManifest =
            context.assets.open("$ASSET_ROOT/runtime-manifest.json").bufferedReader().use { it.readText() }
        val container = File(context.filesDir, ASSET_ROOT)
        return RuntimePlan(
            expectedManifest = expectedManifest,
            destination = destinationForManifest(container, expectedManifest),
        )
    }

    internal fun destinationForManifest(container: File, expectedManifest: String): File =
        File(container, sha256Hex(expectedManifest))

    internal fun runtimeIsReady(destination: File, expectedManifest: String): Boolean {
        if (!readyMarkerMatchesDigest(destination)) {
            return false
        }
        return manifestAndViteMatch(destination, expectedManifest)
    }

    private fun manifestAndViteMatch(destination: File, expectedManifest: String): Boolean {
        val currentManifest = File(destination, "runtime-manifest.json")
            .takeIf(File::isFile)
            ?.readText()
        if (currentManifest != expectedManifest) {
            return false
        }
        val vite = File(destination, REQUIRED_VITE_PATH).toPath()
        return Files.isRegularFile(vite) && !Files.isSymbolicLink(vite)
    }

    private fun recoverReadyMarkerIfInventoryStillMatches(
        destination: File,
        expectedManifest: String,
    ): Boolean {
        if (!manifestAndViteMatch(destination, expectedManifest)) {
            return false
        }
        if (!runtimeInventoryIsValid(destination, expectedManifest)) {
            return false
        }
        return publishReadyMarker(destination)
    }

    internal fun cleanupObsoleteDigests(container: File, keep: String) {
        container.listFiles().orEmpty().forEach { entry ->
            when {
                entry.name == keep -> Unit
                entry.name.endsWith(".staging") -> entry.deleteRecursively()
                isFailureMarkerName(entry.name) && entry.name != failureMarkerForDigest(keep) ->
                    entry.deleteRecursively()
                isReadyMarkerName(entry.name) && entry.name != readyMarkerForDigest(keep) ->
                    entry.deleteRecursively()
                isDigestDirectoryName(entry.name) -> entry.deleteRecursively()
            }
        }
    }

    private fun isDigestDirectoryName(name: String): Boolean =
        name.length == 64 && name.all { it in '0'..'9' || it in 'a'..'f' }

    private fun sha256Hex(text: String): String =
        MessageDigest.getInstance("SHA-256")
            .digest(text.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }

    internal fun runtimeInventoryIsValid(
        root: File,
        manifestJson: String,
        requiredPlatform: String = REQUIRED_PLATFORM,
    ): Boolean {
        val manifest = parseRuntimeManifest(manifestJson, requiredPlatform) ?: return false
        val expectedPaths = manifest.files.mapTo(hashSetOf()) { it.path }.apply { add(RUNTIME_MANIFEST_PATH) }
        val actualPaths =
            root.walkTopDown()
                .filter { it.isFile || Files.isSymbolicLink(it.toPath()) }
                .mapTo(hashSetOf()) { it.relativeTo(root).invariantSeparatorsPath }
        if (actualPaths != expectedPaths) {
            return false
        }
        return manifest.files.all { entry ->
            val current = File(root, entry.path)
            when (entry.kind) {
                "file" ->
                    Files.isRegularFile(current.toPath(), LinkOption.NOFOLLOW_LINKS) &&
                        current.length() == entry.sizeBytes &&
                        sha256Hex(current) == entry.sha256
                "symlink" ->
                    Files.isSymbolicLink(current.toPath()) &&
                        Files.readSymbolicLink(current.toPath()).toString().toByteArray(Charsets.UTF_8).size.toLong() == entry.sizeBytes &&
                        sha256Hex(Files.readSymbolicLink(current.toPath()).toString()) == entry.sha256
                else -> false
            }
        }
    }

    private fun parseRuntimeManifest(
        manifestJson: String,
        requiredPlatform: String,
    ): RuntimeManifest? = runCatching {
        val root = JSONObject(manifestJson)
        if (root.optInt("schema_version", -1) != 1) return@runCatching null
        if (root.optString("platform") != requiredPlatform) return@runCatching null
        if (!root.optBoolean("read_only", false)) return@runCatching null
        val files = root.optJSONArray("files") ?: return@runCatching null
        if (files.length() == 0) return@runCatching null
        val parsedFiles = parseRuntimeManifestFiles(files) ?: return@runCatching null
        if (!hasRequiredInventory(parsedFiles)) return@runCatching null
        RuntimeManifest(files = parsedFiles)
    }.getOrNull()

    private fun parseRuntimeManifestFiles(files: JSONArray): List<RuntimeManifestEntry>? {
        val parsed = mutableListOf<RuntimeManifestEntry>()
        for (index in 0 until files.length()) {
            val entry = files.optJSONObject(index) ?: return null
            val path = entry.optString("path")
            val kind = entry.optString("kind")
            val sha256 = entry.optString("sha256")
            val sizeBytes = entry.optLong("size_bytes", -1L)
            if (
                !isValidManifestPath(path) ||
                kind !in setOf("file", "symlink") ||
                sha256.length != 64 ||
                !sha256.all { it in '0'..'9' || it in 'a'..'f' } ||
                sizeBytes < 0
            ) {
                return null
            }
            parsed += RuntimeManifestEntry(path = path, kind = kind, sha256 = sha256, sizeBytes = sizeBytes)
        }
        return parsed
    }

    private fun isValidManifestPath(path: String): Boolean {
        if (path.isEmpty() || path.startsWith("/") || path.contains('\\')) {
            return false
        }
        return path.split('/').all { component ->
            component.isNotEmpty() && component != "." && component != ".."
        }
    }

    private fun hasRequiredInventory(files: List<RuntimeManifestEntry>): Boolean {
        val paths = files.mapTo(hashSetOf()) { it.path }
        if (!paths.containsAll(REQUIRED_INVENTORY_PATHS)) return false
        val hasRolldownBinding =
            files.any { it.kind == "file" && it.path.startsWith("node_modules/@rolldown/") && it.path.endsWith(".node") }
        val hasLightningCssBinding =
            files.any { it.kind == "file" && it.path.startsWith("node_modules/lightningcss-") && it.path.endsWith(".node") }
        return hasRolldownBinding && hasLightningCssBinding
    }

    internal fun replaceAtomically(
        staging: File,
        destination: File,
        promote: (File, File) -> Boolean = { source, target -> source.renameTo(target) },
    ): Boolean {
        destination.deleteRecursively()
        return promote(staging, destination)
    }

    internal fun failureMarkerFor(destination: File): File {
        val parent = destination.parentFile ?: destination
        return File(parent, failureMarkerForDigest(destination.name))
    }

    internal fun readyMarkerFor(destination: File): File {
        val parent = destination.parentFile ?: destination
        return File(parent, readyMarkerForDigest(destination.name))
    }

    internal fun publishFailureMarker(destination: File, reason: String) {
        runCatching {
            deleteReadyMarker(destination)
            val marker = failureMarkerFor(destination)
            marker.parentFile?.mkdirs()
            val temp = File(marker.parentFile, "${marker.name}.tmp")
            temp.writeText(reason.take(FAILURE_MARKER_MAX_CHARS).trim(), Charsets.UTF_8)
            if (!temp.renameTo(marker)) {
                temp.delete()
            }
        }
    }

    internal fun deleteFailureMarker(destination: File) {
        runCatching { failureMarkerFor(destination).delete() }
    }

    internal fun publishReadyMarker(destination: File): Boolean = runCatching {
        val digest = destination.name
        if (!isDigestDirectoryName(digest)) {
            return@runCatching false
        }
        val marker = readyMarkerFor(destination)
        marker.parentFile?.mkdirs()
        val temp = File(marker.parentFile, "${marker.name}.tmp")
        temp.writeText(digest.take(READY_MARKER_MAX_CHARS), Charsets.UTF_8)
        if (marker.exists() && !marker.delete()) {
            temp.delete()
            return@runCatching false
        }
        if (!temp.renameTo(marker)) {
            temp.delete()
            return@runCatching false
        }
        true
    }.getOrDefault(false)

    internal fun deleteReadyMarker(destination: File) {
        runCatching { readyMarkerFor(destination).delete() }
    }

    private fun readyMarkerMatchesDigest(destination: File): Boolean {
        val digest = destination.name
        if (!isDigestDirectoryName(digest)) {
            return false
        }
        val marker = readyMarkerFor(destination)
        val markerPath = marker.toPath()
        if (!Files.isRegularFile(markerPath) || Files.isSymbolicLink(markerPath)) {
            return false
        }
        if (marker.length() != digest.length.toLong() || marker.length() > READY_MARKER_MAX_CHARS) {
            return false
        }
        return runCatching { marker.readText(Charsets.UTF_8) == digest }.getOrDefault(false)
    }

    internal fun sealRegularFilesReadOnly(root: File) {
        if (!root.exists()) return
        root.walkTopDown().forEach { entry ->
            if (entry.isFile) {
                // The staged seed is host-managed: copied files should not stay
                // writable after extraction, while directories remain traversable
                // so later upgrades can replace the whole tree atomically.
                entry.setReadOnly()
            }
        }
    }

    internal data class RuntimePlan(
        val expectedManifest: String,
        val destination: File,
    )

    private data class RuntimeManifest(
        val files: List<RuntimeManifestEntry>,
    )

    private data class RuntimeManifestEntry(
        val path: String,
        val kind: String,
        val sha256: String,
        val sizeBytes: Long,
    )

    private fun copyAssetTree(context: Context, assetPath: String, destination: File) {
        val children = context.assets.list(assetPath).orEmpty()
        if (children.isEmpty()) {
            destination.parentFile?.mkdirs()
            context.assets.open(assetPath).use { input ->
                destination.outputStream().use(input::copyTo)
            }
            return
        }
        destination.mkdirs()
        children.forEach { child ->
            copyAssetTree(context, "$assetPath/$child", File(destination, child))
        }
    }

    private fun sha256Hex(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        FileInputStream(file).use { input ->
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            while (true) {
                val read = input.read(buffer)
                if (read <= 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    private fun failureMarkerForDigest(digest: String): String = ".${digest}${FAILURE_MARKER_SUFFIX}"
    private fun readyMarkerForDigest(digest: String): String = ".${digest}${READY_MARKER_SUFFIX}"

    private fun isFailureMarkerName(name: String): Boolean =
        name.startsWith(".") &&
            name.endsWith(FAILURE_MARKER_SUFFIX) &&
            isDigestDirectoryName(
                name.removePrefix(".").removeSuffix(FAILURE_MARKER_SUFFIX),
            )

    private fun isReadyMarkerName(name: String): Boolean =
        name.startsWith(".") &&
            name.endsWith(READY_MARKER_SUFFIX) &&
            isDigestDirectoryName(
                name.removePrefix(".").removeSuffix(READY_MARKER_SUFFIX),
            )
}
