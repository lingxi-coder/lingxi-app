package com.lingxi.code.localapps

import android.content.Context
import java.io.File
import java.util.concurrent.Callable
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * What this process knows about the staged local-app runtime.
 *
 * It exists because the root is read exactly ONCE, at engine construction, and
 * the value read then is frozen for the life of the process:
 * `build_mobile_engine_inner` passes `local_apps_runtime_root` into
 * `profile_apps` (engine-mobile/src/host.rs:5447), which memoises the resulting
 * `LocalAppsHostBroker` in a process-global registry keyed on the app files dir
 * (local_apps_profile.rs:149-180 — `cell.get_or_try_init`, and the registry has
 * no removal path), and `LocalAppsHostBroker.runtime_root`
 * (local_apps_host.rs:159) has no setter, so `fixed_runtime_mount` (:235) keeps
 * answering from the first value forever. A `null` handed to the first engine
 * build therefore survives every reconnect and project switch, and only a
 * process restart clears it.
 *
 * Nothing here changes that. It lets the local-apps surface TELL the user which
 * state they are in instead of leaving them with the engine's "stage
 * local-app-runtime first", which is not an instruction a user can follow.
 */
internal enum class LocalAppRuntimeStaging {
    /** Nothing has asked for the runtime yet in this process. */
    Idle,

    /** A staging run is extracting the tree right now. */
    Staging,

    /** Staged, and no caller has yet been handed a `null` root. */
    Ready,

    /**
     * Staged, but a caller was already handed `null` — see the enum doc.
     *
     * The name is the recorded FACT and stops there, because that is all this
     * process holds. It is deliberately not "staged after the engine build":
     * an engine build that took a `null` can still have died before memoising
     * it (`build_mobile_inner_with_ask` is `?`-propagated at host.rs:5422-5430,
     * ahead of the `profile_apps` call at :5447), and the next build then takes
     * the staged root and generates apps normally while this stays latched. So
     * it means "an engine build MAY hold no root" — see
     * [LocalAppRuntimeAssets.noticeFor] for why the copy for this state is
     * conditional where the others are not.
     */
    StagedAfterNullHandout,

    /** A staging run finished and produced no usable runtime. */
    Unavailable,
}

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
     * times over; a cold first launch times out and gets `null`.
     *
     * That `null` is NOT recoverable in-process — see [LocalAppRuntimeStaging]
     * for the two engine-side memos that freeze it — so the staging run keeps
     * going, [stagingStatus] reports where it got to, and [generationDetail]
     * puts that on the screen a failed generation already shows. Widening this
     * budget would only change the odds of hitting it, so it stays where the
     * ANR margin puts it.
     */
    private const val STAGE_BUDGET_MS = 1_500L

    @Volatile
    private var stagedRoot: String? = null

    private val lock = Any()
    private var inFlight: FutureTask<String?>? = null

    /**
     * Set once any caller has been handed `null`. On the production path that
     * caller is `buildVoiceEngine`, so it means an engine may already have been
     * built — and its profile memoised — without a runtime root.
     */
    @Volatile
    private var handedNullToCaller = false

    /**
     * Set when a run finishes with no root. Never cleared on its own: a LIVE
     * run outranks it in [stagingStatus], and a run that succeeds sets
     * [stagedRoot], which outranks both.
     */
    @Volatile
    private var lastRunProducedNothing = false

    fun prepare(context: Context): String? {
        val appContext = context.applicationContext
        return prepareWithin(STAGE_BUDGET_MS) { stage(appContext) }
    }

    /**
     * Where staging got to, from the perspective of a surface that has to
     * explain a failure. Cheap and non-blocking: it only ever calls `get()` on
     * a task that has already completed, so it is safe from a reducer on the
     * main thread.
     */
    internal fun stagingStatus(): LocalAppRuntimeStaging {
        // A run that finishes while nobody is waiting on it leaves its result
        // unclaimed, because [prepareWithin] harvests only on the caller's way
        // out. Harvest here too, or a completed successful run would keep
        // reading as "still staging" and a completed empty one could never be
        // told apart from one still in progress.
        synchronized(lock) { inFlight }?.let(::harvest)
        val root = stagedRoot
        return when {
            root != null && handedNullToCaller -> LocalAppRuntimeStaging.StagedAfterNullHandout
            root != null -> LocalAppRuntimeStaging.Ready
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
     * the runtime is not necessarily the only thing that went wrong. The notice
     * is still true when it is not the proximate cause — with no root in the
     * broker, every generation in this process fails at the Building stage.
     */
    internal fun generationDetail(detail: String?, failed: Boolean): String? {
        if (!failed) return detail
        val notice = noticeFor(stagingStatus()) ?: return detail
        return listOfNotNull(detail?.takeIf(String::isNotBlank), notice).joinToString("\n\n")
    }

    /**
     * Naming the restart is the whole point for [LocalAppRuntimeStaging.Staging]
     * and [LocalAppRuntimeStaging.Unavailable]: both are read with [stagedRoot]
     * still null, so no caller can have received a root and the live engine
     * demonstrably has none — the alternative the user is given today is a
     * retry guaranteed to fail the same way.
     *
     * [LocalAppRuntimeStaging.StagedAfterNullHandout] is weaker and its copy
     * says so. It latches off [handedNullToCaller], which records that a CALLER
     * was handed `null` — NOT that the live engine memoised one. The first
     * engine build can fail before it reaches `profile_apps`
     * (`build_mobile_inner_with_ask` is `?`-propagated at host.rs:5422-5430,
     * ahead of :5447), memoising nothing; the next build then takes the staged
     * root and generates apps normally, while the flag stays set for the life
     * of the process. So this notice states the condition and leaves the
     * engine's own message — which is joined above it by [generationDetail] —
     * to decide, rather than diagnosing a failure it cannot see. The
     * authoritative signal (does the memoised broker hold a root) lives in the
     * engine and is not reachable from here.
     */
    internal fun noticeFor(status: LocalAppRuntimeStaging): String? = when (status) {
        LocalAppRuntimeStaging.Ready, LocalAppRuntimeStaging.Idle -> null
        LocalAppRuntimeStaging.Staging ->
            "本地应用运行时仍在首次准备中。引擎只在启动时读取一次运行时路径，" +
                "因此准备完成后需要重启 App 才能生成应用。"
        LocalAppRuntimeStaging.StagedAfterNullHandout ->
            "本地应用运行时已准备就绪，但本次启动中曾有一次引擎构建没能拿到它。" +
                "引擎只在启动时读取一次运行时路径：如果失败原因是运行时不可用，" +
                "请重启 App 后重试；否则本次失败与运行时无关。"
        LocalAppRuntimeStaging.Unavailable ->
            "本地应用运行时准备失败（安装包中没有运行时，或解压失败），本次启动无法生成应用。"
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
        harvest(task)
        if (staged == null) handedNullToCaller = true
        return staged
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
            handedNullToCaller = false
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
