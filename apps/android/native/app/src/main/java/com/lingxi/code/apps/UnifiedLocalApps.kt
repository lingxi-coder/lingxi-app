package com.lingxi.code.apps

import android.content.Context
import android.os.Build
import com.lingxi.code.BuildConfig
import com.lingxi.code.bindings.android.buildAndroidLocalAppService
import com.lingxi.code.bindings.android.buildAndroidMobileLinuxRuntimeHandle
import com.lingxi.code.bindings.runtime.MobileLocalAppServiceHandle
import com.lingxi.code.project.MobileUserStorage
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Process-owned service. Constructing or browsing the library never needs a conversation. */
class UnifiedLocalApps private constructor(private val context: Context) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mutableState = MutableStateFlow(AppLibraryState())
    val state: StateFlow<AppLibraryState> = mutableState.asStateFlow()
    internal fun recordRuntimeFailure(failure: Exception) {
        mutableState.value = mutableState.value.copy(error = failure.message ?: failure.toString())
    }
    private val creation = Mutex()
    private var handle: MobileLocalAppServiceHandle? = null
    private var linuxBound = false
    private val agentCreation = Mutex()
    private var agentModelHost: com.lingxi.code.bindings.runtime.MobileEngineHandle? = null
    private var agentConfiguration: Pair<String, String?>? = null
    private suspend fun ensureAgentHost() = agentCreation.withLock {
        while (true) {
            val configured = withContext(Dispatchers.IO) {
                com.lingxi.code.settings.ProviderSettingsRepository(context).engineLaunchConfig()
            }
            // A chat's default model/thinking choice is not this app's model.
            val identity = configured.providerProfilesJson to configured.routingJson
            if (agentModelHost != null && agentConfiguration == identity) return@withLock
            withContext(Dispatchers.IO) {
                agentModelHost = com.lingxi.code.voice.buildVoiceEngine(
                    context = context, model = configured.defaultModel,
                    providerProfilesJson = configured.providerProfilesJson, routingJson = configured.routingJson,
                    appAgentHost = true, onEvent = {}, onPermission = {},
                ) ?: error("无法初始化应用的模型服务，请检查模型配置。")
                agentConfiguration = identity
            }
            // Re-read after construction; a concurrent settings change must
            // not let the first page request use the previous configuration.
        }
    }

    val market = UnifiedAppMarket(context, this)
    private val opened = context.getSharedPreferences("local-app-v2-opened", Context.MODE_PRIVATE)
    private val visible = mutableSetOf<android.app.Activity>()
    private val backgroundSettings = Mutex()
    private val browserRuns = mutableMapOf<String, AppLaunch>()
    private val mutableBrowserReviewRequest = MutableStateFlow<BrowserReviewRequest?>(null)
    val browserReviewRequest: StateFlow<BrowserReviewRequest?> = mutableBrowserReviewRequest.asStateFlow()
    fun setBrowserReviewRequest(appId: String, requestId: String?) {
        mutableBrowserReviewRequest.value = BrowserReviewRequest(appId, requestId)
    }
    fun clearBrowserReviewRequest(appId: String, requestId: String?) {
        if (mutableBrowserReviewRequest.value == BrowserReviewRequest(appId, requestId)) mutableBrowserReviewRequest.value = null
    }
    suspend fun openExternalBrowser(id: String): AppLaunch {
        val app = get(id)
        val release = release(id, app.currentRevision)
        check(keepRunning(id, release.digest)) { "请先允许此版本在浏览器使用期间保持本地服务与运行通知。" }
        if ("agent" in release.capabilities) ensureAgentHost()
        // Start a user-visible foreground lease while LingXi is still visible;
        // browser foreground must not cancel its independently owned tasks.
        withContext(Dispatchers.Main.immediate) { LocalAppProcessService.acquire(context, id, app.name) }
        try {
            val launch = start(id)
            browserRuns[id] = launch
            LocalAppProcessService.ready(id)
            return launch
        } catch (failure: Throwable) {
            if (browserRuns[id] == null) LocalAppProcessService.release(context, id)
            throw failure
        }
    }

    init {
        scope.launch { refresh() }
        val application = context.applicationContext as android.app.Application
        application.registerActivityLifecycleCallbacks(object : android.app.Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: android.app.Activity) { visible.add(activity) }
            override fun onActivityStopped(activity: android.app.Activity) {
                visible.remove(activity)
                scope.launch {
                    // Configuration changes replace the Activity immediately;
                    // native pickers temporarily present a different app.
                    kotlinx.coroutines.delay(700)
                    if (visible.isNotEmpty()) return@launch
                    try {
                        for (app in (request("list") as JSONArray).let { list -> (0 until list.length()).map { list.getJSONObject(it).appRecord() } }) {
                            if (visible.isNotEmpty()) break
                            if (browserRuns.containsKey(app.id)) continue
                            val running = (request("status", app.id) as JSONObject).optJSONObject("run") ?: continue
                            val release = release(app.id, running.getLong("revision"))
                            if (visible.isEmpty() && release.executesCode && !keepRunning(app.id, release.digest)) runCatching { suspend(app.id) }
                        }
                    } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
                    catch (failure: Exception) { mutableState.value = mutableState.value.copy(error = failure.message) }
                }
            }
            override fun onActivityCreated(activity: android.app.Activity, state: android.os.Bundle?) = Unit
            override fun onActivityResumed(activity: android.app.Activity) = Unit
            override fun onActivityPaused(activity: android.app.Activity) = Unit
            override fun onActivitySaveInstanceState(activity: android.app.Activity, state: android.os.Bundle) = Unit
            override fun onActivityDestroyed(activity: android.app.Activity) { visible.remove(activity) }
        })
    }

    private suspend fun service(): MobileLocalAppServiceHandle = creation.withLock {
        handle ?: withContext(Dispatchers.IO) {
            val directories = MobileUserStorage.directories(context)
            // Browsing and static apps are available even when Linux assets are missing.
            buildAndroidLocalAppService(directories.configHome, null).also { created ->
                decodeAppResponse(created.requestJson(JSONObject().put("op", "configureTarget").put("target", appTarget(if (BuildConfig.MOBILE_LINUX_FULL) "mobile-linux" else "web")).toString()))
            }
        }.also { handle = it }
    }
    /** Trusted app-details UI only; no WebView message can reach this method. */
    suspend fun agentHost(request: JSONObject): Any? = withContext(Dispatchers.IO) {
        ensureAgentHost()
        decodeAppResponse(service().agentHostJson(request.toString()))
    }
    /** Trusted native authoring only. This registration must not boot the model host. */
    suspend fun registerAuthoring(authoring: JSONObject): AppAuthoring = withContext(Dispatchers.IO) {
        val request = JSONObject().put("op", "registerAuthoring").put("authoring", authoring)
        (decodeAppResponse(service().agentHostJson(request.toString())) as JSONObject).appAuthoring()
    }
    suspend fun agentTasks(appId: String, revision: Long): List<AppAgentTask> {
        val response = agentHost(JSONObject().put("op", "tasks.list").put("appId", appId).put("revision", revision)) as JSONObject
        require(response.optString("appId") == appId && response.optLong("revision") == revision) {
            "The Agent task list belongs to a different application release."
        }
        val tasks = response.getJSONArray("tasks")
        return (0 until tasks.length()).map { tasks.getJSONObject(it).appAgentTask() }
    }
    suspend fun cancelAgentTask(appId: String, revision: Long, runId: String) {
        agentHost(JSONObject().put("op", "tasks.cancel").put("appId", appId).put("revision", revision).put("runId", runId))
    }
    private suspend fun bindLinux() = creation.withLock {
        check(BuildConfig.MOBILE_LINUX_FULL) { "此发行渠道不支持 Linux 应用代码执行。" }
        if (linuxBound) return@withLock
        withContext(Dispatchers.IO) {
            val directories = MobileUserStorage.directories(context)
            val linux = buildAndroidMobileLinuxRuntimeHandle(LinuxRuntimeBridge.configForWorkspace(context, LinuxRuntimeMode.MobileLinux))
            // The Rust registry fills the existing owner's lazy runtime slot; it does
            // not replace current app runs, the library or the persistent origin.
            buildAndroidLocalAppService(directories.configHome, linux)
        }
        linuxBound = true
    }

    internal suspend fun request(op: String, appId: String? = null, fields: JSONObject.() -> Unit = {}): Any? {
        val json = JSONObject().put("op", op).apply { appId?.let { put("appId", it) }; fields() }
        val service = service()
        return withContext(Dispatchers.IO) { decodeAppResponse(service.requestJson(json.toString())) }
    }
    suspend fun refresh() {
        try {
            val rows = request("list") as JSONArray
            mutableState.value = AppLibraryState((0 until rows.length()).map { rows.getJSONObject(it).appRecord() }, false)
        } catch (e: kotlinx.coroutines.CancellationException) { throw e }
        catch (e: Exception) { mutableState.value = mutableState.value.copy(loading = false, error = e.message ?: e.toString()) }
    }
    suspend fun get(id: String) = (request("get", id) as JSONObject).appRecord()
    suspend fun releases(id: String): List<AppRelease> = (request("releases", id) as JSONArray).let { values ->
        (0 until values.length()).map { values.getJSONObject(it).appRelease() }
    }
    suspend fun release(id: String, revision: Long) = (request("getRelease", id) { put("revision", revision) } as JSONObject).appRelease()
    suspend fun grant(id: String, digest: String) { request("grant", id) { put("digest", digest) }; refresh() }
    suspend fun revoke(id: String) {
        browserRuns.remove(id)
        request("stop", id)
        LocalAppProcessService.release(context, id)
        request("revoke", id)
        refresh()
    }
    suspend fun start(id: String, op: String = "start", revision: Long? = null): AppLaunch {
        val app = get(id)
        val release = release(id, revision ?: app.currentRevision)
        val environment = if (release.executesCode) "mobile-linux" else "web"
        val compatible = request("checkTarget", id) { put("revision", release.revision); put("target", appTarget(environment)) } as JSONObject
        check(compatible.getBoolean("compatible")) { "此版本不支持当前设备或所需能力。" }
        if (release.executesCode) {
            check(BuildConfig.MOBILE_LINUX_FULL) { "此发行渠道不支持安装、构建或运行应用服务。" }
            require(release.digest in app.executionGrants) { "请先允许此版本在 Linux 用户空间中运行。" }
            bindLinux()
            if (keepRunning(id, release.digest)) withContext(Dispatchers.Main.immediate) { LocalAppProcessService.acquire(context, id, app.name) }
        }
        val value = try { request(op, id) { revision?.let { put("revision", it) } } as JSONObject } catch (error: Throwable) {
            // The shared service cancels only this preparation. An app-level
            // Stop here would also terminate its still-serving previous run.
            withContext(kotlinx.coroutines.NonCancellable) {
                val retained = runCatching { (request("status", id) as JSONObject).optJSONObject("run")?.optString("phase") == "running" }.getOrDefault(false)
                if (retained) LocalAppProcessService.ready(id) else LocalAppProcessService.release(context, id)
            }
            throw error
        }
        val launch = AppLaunch(value.getJSONObject("run").appRun(), value.getString("url"), value.getString("origin"))
        require(launch.run.appId == id && launch.run.phase == "running") { "Application did not become ready" }
        LocalAppProcessService.ready(id)
        return launch
    }
    suspend fun stop(id: String) { browserRuns.remove(id); request("stop", id); LocalAppProcessService.release(context, id); refresh() }
    suspend fun suspend(id: String) { browserRuns.remove(id); request("suspend", id); LocalAppProcessService.release(context, id) }
    suspend fun repair(id: String) {
        start(id, "repair")
        refresh()
    }
    suspend fun activate(id: String, revision: Long) {
        start(id, revision = revision)
        refresh()
    }
    suspend fun runs(id: String): List<AppRun> = (request("runs", id) as JSONArray).let { values ->
        (0 until values.length()).map { values.getJSONObject(it).appRun() }
    }
    suspend fun logs(id: String, runId: String): String = request("logs", id) { put("runId", runId) }.let { value ->
        if (value is JSONArray) (0 until value.length()).joinToString("\n") { index ->
            val item = value.optJSONObject(index)
            item?.optionalString("text") ?: value.get(index).toString()
        } else value?.toString().orEmpty()
    }
    suspend fun remove(id: String) { stop(id); request("remove", id); refresh() }
    suspend fun prepareAuthoring(id: String, sessions: Map<String, AppAuthoringSessionContext>): AppAuthoringResolution {
        val app = get(id)
        val source = app.authoring?.sourceDirectory
        val exists = source != null && withContext(Dispatchers.IO) { hostFile(source).isDirectory }
        val sessionBindings = JSONObject().apply { sessions.forEach { (id, binding) -> put(id, binding.toJson()) } }
        val value = request("prepareAuthoring", id) { put("sessionBindings", sessionBindings); put("sourceExists", exists) } as JSONObject
        return AppAuthoringResolution(value.getString("action"), value.getString("sourceDirectory"), value.optJSONObject("authoring")?.appAuthoring())
    }
    suspend fun importSource(sourcePath: String, marketOrigin: JSONObject? = null): AppRecord {
        val value = request("import") { put("sourcePath", sourcePath); marketOrigin?.let { put("marketOrigin", it) } } as JSONObject
        refresh()
        return (value.optJSONObject("record") ?: value).appRecord()
    }
    suspend fun createAuthoring(name: String): String = (request("createAuthoring") { put("name", name) } as JSONObject).getString("sourceDirectory")
    suspend fun export(id: String, revision: Long, destination: String) { request("export", id) { put("revision", revision); put("destination", destination) } }
    suspend fun sourceArchive(id: String, revision: Long) = sourceArchive(id, revision, context.cacheDir)
    suspend fun importArchive(uri: android.net.Uri): AppRecord = withContext(Dispatchers.IO) {
        val folder = File(context.cacheDir, "local-app-source-import/${java.util.UUID.randomUUID()}")
        check(folder.mkdirs()) { "Unable to prepare source import" }
        try {
            val archive = File(folder, "source.zip")
            context.contentResolver.openInputStream(uri)?.use { input -> archive.outputStream().use { output ->
                val bytes = ByteArray(32768); var size = 0L
                while (true) { kotlinx.coroutines.currentCoroutineContext().ensureActive(); val read = input.read(bytes); if (read < 0) break; size += read; require(size <= 25L * 1024 * 1024) { "Source archive exceeds 25 MB" }; output.write(bytes, 0, read) }
            } } ?: error("Cannot read source archive")
            val source = File(folder, "source"); check(source.mkdir())
            unpackLocalAppSource(archive, source)
            importSource(source.absolutePath)
        } finally { folder.deleteRecursively() }
    }
    fun guestPath(hostPath: String): String {
        val directories = MobileUserStorage.directories(context)
        val home = File(directories.home).canonicalFile.toPath()
        val path = File(hostPath).canonicalFile.toPath()
        require(path.startsWith(home)) { "Not a Linux user directory" }
        return directories.guestHome + "/" + home.relativize(path).toString()
    }
    fun keepRunning(id: String, digest: String?) = digest != null && opened.getBoolean("background:$id:$digest", false)
    suspend fun setKeepRunning(id: String, digest: String, value: Boolean) = backgroundSettings.withLock {
        withContext(Dispatchers.Main.immediate) {
            val previous = keepRunning(id, digest)
            val hadLease = LocalAppProcessService.owns(id)
            var acquiredLease = false
            val appName = get(id).name
            val running = (request("status", id) as JSONObject).optJSONObject("run")
            val ownsRun = running?.optString("releaseDigest") == digest && running.optString("phase") == "running"
            val code = ownsRun && release(id, running!!.getLong("revision")).executesCode
            if (value && code) {
                check(visible.isNotEmpty()) { "Open LingXi before enabling background execution" }
                LocalAppProcessService.acquire(context, id, appName)
                LocalAppProcessService.ready(id)
                acquiredLease = !hadLease
            }
            try {
                check(withContext(Dispatchers.IO) { opened.edit().putBoolean("background:$id:$digest", value).commit() }) { "Unable to save background preference" }
            } catch (failure: Throwable) {
                // commit() updates memory even when disk persistence fails.
                // Restore the effective preference and only undo our new lease.
                opened.edit().putBoolean("background:$id:$digest", previous).apply()
                if (acquiredLease) LocalAppProcessService.release(context, id)
                throw failure
            }
            if (!value && ownsRun) {
                LocalAppProcessService.release(context, id)
                if (code && visible.isEmpty()) suspend(id)
            }
        }
    }
    suspend fun restoreSource(id: String, remix: Boolean): AppRecord {
        val guest = MobileUserStorage.directories(context).appsGuestPath.trimEnd('/') + "/" + java.util.UUID.randomUUID()
        val source = hostFile(guest)
        val value = request(if (remix) "remix" else "restoreAuthoring", id) {
            put("sourcePath", source.absolutePath)
            put("authoring", JSONObject().put("sourceDirectory", guest))
        } as JSONObject
        refresh()
        return (value.optJSONObject("record") ?: value).appRecord()
    }
    fun markOpened(id: String) { opened.edit().putLong(id, System.currentTimeMillis()).apply(); mutableState.value = mutableState.value.copy(openedRevision = mutableState.value.openedRevision + 1) }
    fun drawerApps(records: List<AppRecord>) = records.map { MyApp(it.id, it.name, "", opened.getLong(it.id, 0).takeIf { time -> time > 0 }) }
    internal fun legacyMarketRoot(): File = File(context.filesDir, "local-app-market")
    internal fun newLegacyImportGuestPath(): String =
        MobileUserStorage.directories(context).appsGuestPath.trimEnd('/') + "/import-${java.util.UUID.randomUUID()}"
    fun hostFile(guestPath: String): File {
        val dirs = MobileUserStorage.directories(context)
        require(guestPath.startsWith(dirs.guestHome + "/") && guestPath.split('/').none { it == "." || it == ".." }) { "Invalid Linux user directory" }
        val home = File(dirs.home).canonicalFile
        return File(home, guestPath.removePrefix(dirs.guestHome + "/")).canonicalFile.also {
            require(it.toPath().startsWith(home.toPath())) { "Directory escaped Linux user home" }
        }
    }
    companion object {
        @Volatile private var instance: UnifiedLocalApps? = null
        fun get(context: Context): UnifiedLocalApps = instance ?: synchronized(this) {
            instance ?: UnifiedLocalApps(context.applicationContext).also { instance = it }
        }
    }
}

internal fun appTarget(environment: String): JSONObject = JSONObject().put("platform", "android").put("environment", environment)
    .put("architecture", canonicalAppArchitecture(Build.SUPPORTED_ABIS.firstOrNull()))
    .put("runtimeVersion", BuildConfig.MOBILE_LINUX_SDK_VERSION)
    .put("channels", JSONArray(listOf(BuildConfig.FLAVOR)))
    .put("capabilities", JSONArray((setOf("camera", "photos", "files", "microphone", "clipboard", "share", "network", "agent") + if (BuildConfig.MOBILE_LINUX_FULL) setOf("process", "background") else emptySet()).toList()))
