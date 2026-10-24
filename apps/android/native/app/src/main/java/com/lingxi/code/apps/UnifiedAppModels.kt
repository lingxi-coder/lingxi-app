package com.lingxi.code.apps

import org.json.JSONArray
import org.json.JSONObject
import java.net.URI

/** Schema-2 identities are independent from market IDs and conversation IDs. */
data class AppAuthoring(val sourceDirectory: String, val sourceSessionId: String?, val projectId: String?)
data class AppMarketOrigin(val appId: String, val releaseId: String, val digest: String, val remixedFrom: String? = null)
data class AppRecord(
    val id: String, val name: String, val summary: String,
    val currentRevision: Long, val latestRevision: Long, val authoring: AppAuthoring?,
    val marketOrigin: AppMarketOrigin?, val originPort: Int, val executionGrants: Set<String>,
    val updatedAtMs: Long,
)
data class AppRelease(val appId: String, val revision: Long, val digest: String, val manifest: JSONObject, val marketOrigin: AppMarketOrigin? = null) {
    val capabilities: Set<String> get() = manifest.optJSONArray("capabilities").strings().toSet()
    val executesCode: Boolean get() = "process" in capabilities || listOf("install", "build", "processes").any { (manifest.optJSONArray(it)?.length() ?: 0) > 0 }
}
data class AppRun(val appId: String, val runId: String, val revision: Long, val digest: String, val phase: String, val error: String?, val services: Set<String>)
data class AppLaunch(val run: AppRun, val url: String, val origin: String)
data class AppAuthoringResolution(val action: String, val sourceDirectory: String, val sessionId: String?)
data class AppLibraryState(val apps: List<AppRecord> = emptyList(), val loading: Boolean = true, val error: String? = null, val openedRevision: Long = 0)
class AppServiceException(val code: String, message: String) : Exception(message)

internal fun JSONObject.optionalString(key: String): String? = if (!has(key) || isNull(key)) null else getString(key)
internal fun JSONArray?.strings(): List<String> = if (this == null) emptyList() else (0 until length()).map(::getString)
internal fun JSONObject.appRecord(): AppRecord = AppRecord(
    getString("appId"), getString("name"), getString("summary"), getLong("currentRevision"), getLong("latestRevision"),
    optJSONObject("authoring")?.let { AppAuthoring(it.getString("sourceDirectory"), it.optionalString("sourceSessionId"), it.optionalString("projectId")) },
    optJSONObject("marketOrigin")?.let { AppMarketOrigin(it.getString("appId"), it.getString("releaseId"), it.getString("digest"), it.optionalString("remixedFrom")) },
    getInt("originPort"), getJSONArray("executionGrants").strings().toSet(), getLong("updatedAtMs"),
)
internal fun JSONObject.appRelease() = AppRelease(getString("appId"), getLong("revision"), getString("digest"), getJSONObject("manifest"),
    optJSONObject("marketOrigin")?.let { AppMarketOrigin(it.getString("appId"), it.getString("releaseId"), it.getString("digest"), it.optionalString("remixedFrom")) })
internal fun JSONObject.appRun() = AppRun(getString("appId"), getString("runId"), getLong("revision"), getString("releaseDigest"), getString("phase"), optionalString("error"), getJSONArray("services").let { a -> (0 until a.length()).map { a.getJSONObject(it).getString("name") }.toSet() })
internal fun decodeAppResponse(raw: String): Any? {
    val result = JSONObject(raw)
    if (!result.getBoolean("ok")) {
        val error = result.getJSONObject("error")
        throw AppServiceException(error.getString("code"), error.getString("message"))
    }
    return result.opt("value").takeUnless { it === JSONObject.NULL }
}

/** URL checks are independent of UI objects so stale/native callers can be tested. */
internal fun isCurrentAppFrame(launch: AppLaunch, sourceOrigin: String, pageUrl: String?, mainFrame: Boolean): Boolean = runCatching {
    if (!mainFrame || launch.run.phase != "running" || pageUrl == null) return false
    val origin = URI(launch.origin)
    val page = URI(pageUrl)
    val source = URI(sourceOrigin)
    val entry = URI(launch.url)
    fun sameOrigin(value: URI) = value.scheme == "http" && value.host == "127.0.0.1" && value.port == origin.port && value.userInfo == null
    val runPrefix = "/__run/${launch.run.runId}/"
    sameOrigin(origin) && sameOrigin(source) && sameOrigin(page) && sameOrigin(entry) &&
        page.path.startsWith(runPrefix) && runPrefix.contains(launch.run.runId) &&
        page.path.split('/').none { it == ".." || it == "." } && !page.path.contains('\\') && !page.path.contains('\u0000') &&
        !Regex("%2f|%5c", RegexOption.IGNORE_CASE).containsMatchIn(page.rawPath)
}.getOrDefault(false)

internal fun appServiceUrl(launch: AppLaunch, name: String, path: String, socket: Boolean): String {
    require(name in launch.run.services) { "Unknown application service" }
    require(path.startsWith('/') && !path.startsWith("//") && !path.contains('\\') && !path.contains('#')) { "Invalid service path" }
    val parsed = URI(path)
    require(parsed.path.split('/').none { it == "." || it == ".." } && !parsed.path.contains('\\') && !parsed.path.contains('\u0000') &&
        !Regex("%2f|%5c", RegexOption.IGNORE_CASE).containsMatchIn(parsed.rawPath)) { "Invalid service path" }
    val scheme = if (socket) "ws" else "http"
    return "$scheme://127.0.0.1:${URI(launch.origin).port}/__run/${launch.run.runId}/__service/$name$path"
}

/** Public target architecture is independent from Android packaging ABI names. */
internal fun canonicalAppArchitecture(androidAbi: String?): String = when (androidAbi) {
    "arm64-v8a" -> "aarch64"
    "x86_64" -> "x86_64"
    else -> androidAbi ?: "unknown"
}
