package com.lingxi.code.market

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.security.MessageDigest
import kotlin.coroutines.coroutineContext

data class MarketDistribution(val appId: String, val versionId: String, val sequence: Long, val format: String, val sizeBytes: Long, val sha256: String, val downloadUrl: String)
class MarketException(val code: String, message: String = code) : Exception(message)

/** Anonymous transport. Supply an HTTPS API root including /api/v2. */
open class MarketClient(val apiRoot: String) {
    init { require(URI(apiRoot).scheme == "https" && URI(apiRoot).host != null && URI(apiRoot).userInfo == null && URI(apiRoot).fragment == null) }
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
