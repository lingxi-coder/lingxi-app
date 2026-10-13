package com.lingxi.code.market

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.net.URLEncoder
import java.security.MessageDigest
import kotlin.coroutines.coroutineContext

data class MarketVersion(val id: String, val appId: String, val sequence: Long, val format: String, val filename: String, val sizeBytes: Long?, val sha256: String?, val notes: String, val publishedAt: String?)
data class MarketApp(val id: String, val name: String, val summary: String, val category: String, val platforms: List<String>, val coverUrl: String?, val currentVersion: MarketVersion?, val updatedAt: String)
data class MarketPage<T>(val items: List<T>, val nextCursor: String?)
data class MarketDistribution(val appId: String, val versionId: String, val sequence: Long, val format: String, val sizeBytes: Long, val sha256: String, val downloadUrl: String)
enum class UpdateStatus { UP_TO_DATE, UPDATE_AVAILABLE, UNKNOWN_VERSION, UNAVAILABLE }
data class MarketUpdate(val appId: String, val status: UpdateStatus, val currentVersionId: String?, val currentSequence: Long?)
class MarketException(val code: String, message: String = code) : Exception(message)
internal fun JSONObject.nullString(key: String) = if (isNull(key)) null else getString(key)
internal fun JSONObject.version() = MarketVersion(getString("id"), getString("appId"), getLong("sequence"), getString("format"), getString("filename"), if(isNull("sizeBytes")) null else getLong("sizeBytes"), nullString("sha256"), optString("notes"), nullString("publishedAt"))
internal fun JSONObject.app() = MarketApp(getString("id"), getString("name"), getString("summary"), getString("category"), getJSONArray("platforms").let { a -> (0 until a.length()).map(a::getString) }, nullString("coverUrl"), optJSONObject("currentVersion")?.version(), getString("updatedAt"))

/** Anonymous transport. Supply an HTTPS API root including /api/v1. */
open class MarketClient(val apiRoot: String) {
    init { require(URI(apiRoot).scheme == "https" && URI(apiRoot).host != null && URI(apiRoot).userInfo == null && URI(apiRoot).fragment == null) }
    private fun encode(s: String) = URLEncoder.encode(s, "UTF-8")
    internal open fun connection(address: String): HttpURLConnection {
        var url = URL(address)
        repeat(6) {
            if(url.protocol != "https" || url.userInfo != null) throw MarketException("INSECURE_URL")
            val c = url.openConnection() as HttpURLConnection
            c.instanceFollowRedirects = false; c.connectTimeout = 15000; c.readTimeout = 30000
            if(c.responseCode in listOf(301,302,303,307,308)) {
                val location = c.getHeaderField("Location") ?: throw MarketException("INVALID_REDIRECT")
                url = URL(url, location); c.disconnect()
            } else return c
        }
        throw MarketException("TOO_MANY_REDIRECTS")
    }
    protected open suspend fun request(path: String, body: JSONObject? = null): JSONObject = withContext(Dispatchers.IO) {
        val c = if(body == null) connection(apiRoot.trimEnd('/') + path) else (URL(apiRoot.trimEnd('/') + path).openConnection() as HttpURLConnection).apply {
            instanceFollowRedirects = false; connectTimeout = 15000; readTimeout = 30000
            requestMethod = "POST"; doOutput = true; setRequestProperty("Content-Type", "application/json")
            outputStream.use { it.write(body.toString().toByteArray()) }
        }
        try {
            val ok = c.responseCode in 200..299
            val bytes = (if(ok) c.inputStream else c.errorStream)?.use { input ->
                val output = java.io.ByteArrayOutputStream(); val buffer = ByteArray(8192)
                while(true) { coroutineContext.ensureActive(); val n=input.read(buffer); if(n<0) break; if(output.size()+n>2*1024*1024) throw MarketException("RESPONSE_TOO_LARGE"); output.write(buffer,0,n) }; output.toByteArray()
            } ?: ByteArray(0)
            val json = runCatching { JSONObject(String(bytes, Charsets.UTF_8)) }.getOrElse { throw MarketException("HTTP_${c.responseCode}") }
            if(!ok) throw MarketException(json.optJSONObject("error")?.optString("code")?.takeIf { it.isNotEmpty() } ?: "HTTP_${c.responseCode}")
            json
        } finally { c.disconnect() }
    }
    open suspend fun catalog(query: String = "", cursor: String? = null): MarketPage<MarketApp> {
        val q=query.trim(); val params=mutableListOf("platform=android", "limit=24", "sort=" + if(q.isEmpty()) "updated" else "name")
        if(q.isNotEmpty()) params += "q=${encode(q)}"; cursor?.let { params += "cursor=${encode(it)}" }
        val r=request("/apps?"+params.joinToString("&")); val a=r.getJSONArray("data")
        return MarketPage((0 until a.length()).map { a.getJSONObject(it).app() }.distinctBy { it.id }, r.optJSONObject("pagination")?.nullString("nextCursor"))
    }
    open suspend fun detail(appId: String) = request("/apps/${encode(appId)}").getJSONObject("data").app()
    suspend fun versions(appId: String, cursor: String? = null): MarketPage<MarketVersion> {
        val r=request("/apps/${encode(appId)}/versions?limit=24"+(cursor?.let { "&cursor=${encode(it)}" } ?: "")); val a=r.getJSONArray("data")
        return MarketPage((0 until a.length()).map { a.getJSONObject(it).version() },r.optJSONObject("pagination")?.nullString("nextCursor"))
    }
    suspend fun distribution(appId: String, versionId: String): MarketDistribution {
        val r=request("/apps/${encode(appId)}/versions/${encode(versionId)}/distribution").getJSONObject("data"); val a=r.getJSONObject("artifact")
        if(r.getString("appId")!=appId || r.getString("versionId")!=versionId) throw MarketException("IDENTITY_MISMATCH")
        return MarketDistribution(appId,versionId,r.getLong("sequence"),a.getString("format"),a.getLong("sizeBytes"),a.getString("sha256"),a.getString("downloadUrl"))
    }
    suspend fun updates(installed: List<InstalledApp>): List<MarketUpdate> = installed.chunked(100).flatMap { chunk ->
        val a=request("/updates/check",JSONObject().put("apps",JSONArray(chunk.map { JSONObject().put("appId",it.appId).put("installedVersionId",it.versionId) }))).getJSONArray("data")
        (0 until a.length()).map { a.getJSONObject(it).let { r -> MarketUpdate(r.getString("appId"),UpdateStatus.valueOf(r.getString("status").uppercase()),r.nullString("currentVersionId"),if(r.isNull("currentSequence")) null else r.getLong("currentSequence")) } }
    }
    open suspend fun download(d: MarketDistribution, target: File, progress: (Long,Long)->Unit) = withContext(Dispatchers.IO) {
        if(d.sizeBytes !in 1..25L*1024*1024 || !d.sha256.matches(Regex("[a-fA-F0-9]{64}"))) throw MarketException("INVALID_ARTIFACT")
        val c=connection(d.downloadUrl)
        try {
            if(c.responseCode !in 200..299) throw MarketException("HTTP_${c.responseCode}")
            val digest=MessageDigest.getInstance("SHA-256"); var bytes=0L
            c.inputStream.use { input -> target.outputStream().use { out -> val b=ByteArray(32768); while(true) {
                coroutineContext.ensureActive(); val n=input.read(b); if(n<0) break; bytes+=n
                if(bytes>d.sizeBytes) throw MarketException("SIZE_MISMATCH"); digest.update(b,0,n); out.write(b,0,n); progress(bytes,d.sizeBytes)
            } } }
            if(bytes!=d.sizeBytes) throw MarketException("SIZE_MISMATCH")
            if(digest.digest().joinToString("") { "%02x".format(it) }!=d.sha256.lowercase()) throw MarketException("HASH_MISMATCH")
        } finally { c.disconnect() }
    }
}
