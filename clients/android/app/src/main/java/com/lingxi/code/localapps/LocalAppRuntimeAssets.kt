package com.lingxi.code.localapps

import android.content.Context
import java.io.File
import java.util.concurrent.Callable
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/** Stages the build-verified runtime asset into the app-private filesystem. */
object LocalAppRuntimeAssets {
    private const val ASSET_ROOT = "local-app-runtime"

    /**
     * Every caller reaches this from `buildVoiceEngine`, whose sole production
     * call site is the `viewModel { initializer { … } }` block that Compose
     * runs synchronously on the MAIN thread. Extracting the ~200 MB runtime
     * takes tens of seconds there, well past the 5 s input-dispatch ANR, so the
     * work runs on a background thread and the caller waits only this long.
     * The budget covers the warm path (two manifest reads and a compare) many
     * times over; a cold first launch times out, gets `null` — which the engine
     * already treats as "local apps unavailable", not as a failure — and picks
     * the staged root up on the next engine build.
     */
    private const val STAGE_BUDGET_MS = 1_500L

    @Volatile
    private var stagedRoot: String? = null

    private val lock = Any()
    private var inFlight: FutureTask<String?>? = null

    fun prepare(context: Context): String? {
        val appContext = context.applicationContext
        return prepareWithin(STAGE_BUDGET_MS) { stage(appContext) }
    }

    /**
     * Single-flight staging with a bounded wait. A staging run that outlives
     * the budget keeps going: a later call joins the same task and observes its
     * result, so the tree is never staged twice concurrently. A run that yields
     * `null` is not memoised, so a transient IO error retries on the next
     * engine build.
     */
    internal fun prepareWithin(budgetMs: Long, stage: () -> String?): String? {
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
        if (task.isDone) {
            synchronized(lock) { if (inFlight === task) inFlight = null }
            staged?.let { stagedRoot = it }
        }
        return staged
    }

    /**
     * Staging an OPTIONAL runtime must never take the engine with it: every
     * caller sits inside `buildVoiceEngine`, whose catch-all turns a throw here
     * into "no engine at all" — chat, sessions and cron included. A failure
     * means local apps are unavailable, and it is not memoised, so a transient
     * IO error retries on the next engine build.
     */
    private fun stage(context: Context): String? = runCatching {
        val assetEntries = context.assets.list(ASSET_ROOT).orEmpty()
        if (assetEntries.isEmpty()) return@runCatching null
        val destination = File(context.filesDir, ASSET_ROOT)
        val expectedManifest =
            context.assets.open("$ASSET_ROOT/runtime-manifest.json").bufferedReader().use { it.readText() }
        val currentManifest = File(destination, "runtime-manifest.json")
            .takeIf(File::isFile)
            ?.readText()
        if (currentManifest != expectedManifest) {
            // filesDir, not cacheDir: the rename is then a same-mount atomic
            // move, so a copy that dies mid-tree can never leave a destination
            // whose manifest matches a partially staged runtime.
            val staging = File(context.filesDir, "$ASSET_ROOT-staging")
            staging.deleteRecursively()
            staging.mkdirs()
            copyAssetTree(context, ASSET_ROOT, staging)
            destination.deleteRecursively()
            if (!staging.renameTo(destination)) {
                staging.copyRecursively(destination, overwrite = true)
                staging.deleteRecursively()
            }
        }
        destination
            .takeIf { File(it, "node_modules/next/dist/bin/next").isFile }
            ?.absolutePath
    }.getOrNull()

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
}
