package com.lingxi.code.apps

import android.content.Context
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.market.MarketException
import com.lingxi.code.project.MobileUserStorage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.UUID
import kotlin.coroutines.coroutineContext

data class AppMarketPublication internal constructor(
    val appId: String, val revision: Long, val digest: String, val name: String, val summary: String,
    val files: List<String>, internal val archive: File, internal val sha256: String, internal val uid: String,
)

/** Account state is used only for authoring uploads, never to select the local application HOME. */
class AppMarketPublisher(private val context: Context, private val apps: UnifiedLocalApps, private val market: UnifiedAppMarket) {
    private val serial = Mutex()
    private val preferences = AppPublicationReceipts(File(MobileUserStorage.directories(context).configHome, "local-app-market-publishing"))
    private val auth get() = FirebaseAuth.getInstance()
    private fun key(uid: String, id: String) = sha("$uid:$id".toByteArray())
    private fun save(name: String, value: String) = preferences.save(name, value)
    private fun mutationKey(name: String): String = preferences.getString(name, null) ?: UUID.randomUUID().toString().also { save(name, it) }
    private fun checkAccount(uid: String) { check(auth.currentUser?.uid == uid) { "Publishing account changed; review the source again" } }

    suspend fun prepare(id: String): AppMarketPublication = serial.withLock { withContext(Dispatchers.IO) {
        val user = auth.currentUser ?: throw MarketException("SIGN_IN_REQUIRED", "请先登录市场作者账号；本机应用不需要登录。")
        market.capabilities()
        val record = apps.get(id)
        val release = apps.release(id, record.currentRevision)
        val name = release.manifest.getString("name")
        val summary = release.manifest.optString("summary")
        require(summary.isNotBlank()) { "请先在应用清单中添加简介并发布到本机。" }
        val exported = apps.sourceArchive(id, release.revision, context.cacheDir)
        try {
            checkAccount(user.uid)
            val cache = File(MobileUserStorage.directories(context).configHome, "local-app-market-publishing/${key(user.uid, id)}/${release.digest}")
            check(cache.isDirectory || cache.mkdirs()) { "Unable to save prepared publication" }
            val archive = File(cache, "source.zip")
            val hashKey = "${key(user.uid, id)}:${release.digest}:archiveHash"
            val releaseKey = "${key(user.uid, id)}:${release.digest}"
            val issuedVersion = preferences.contains("$releaseKey:createVersion") || preferences.contains("$releaseKey:versionId")
            if (archive.exists() && preferences.contains(hashKey)) {
                check(preferences.getString(hashKey, null) == sha(archive)) { "Prepared public archive changed; preserve it and publish a new local release" }
            } else {
                check(!issuedVersion) { "Pending public archive is missing its receipt; preserve it for recovery" }
                // A crash between copying and saving the hash cannot have sent a
                // version mutation. Recreate only this unsubmitted archive.
                exported.archive.copyTo(archive, overwrite = true)
                java.io.RandomAccessFile(archive, "rw").use { it.fd.sync() }
                save(hashKey, sha(archive))
            }
            AppMarketPublication(id, release.revision, release.digest, name, summary, exported.files, archive, sha(archive), user.uid)
        } finally { exported.close() }
    } }

    suspend fun publish(prepared: AppMarketPublication): String = serial.withLock { withContext(Dispatchers.IO) {
        checkAccount(prepared.uid)
        val record = apps.get(prepared.appId)
        check(record.currentRevision == prepared.revision && apps.release(record.id, prepared.revision).digest == prepared.digest) { "Application version changed; review its source again" }
        check(sha(prepared.archive) == prepared.sha256) { "Reviewed source archive changed" }
        val owner = key(prepared.uid, prepared.appId)
        val publication = "$owner:${prepared.digest}"
        var release = publication + (preferences.getString("$publication:attempt", null)?.let { ":$it" } ?: "")
        var remote = preferences.getString("$owner:appId", null)
        if (remote == null) {
            val body = preferences.getString("$owner:createBody", null)?.let(::JSONObject) ?: JSONObject()
                .put("name", prepared.name.take(60)).put("summary", prepared.summary.take(160)).put("category", "other")
                .also { save("$owner:createBody", it.toString()) }
            remote = write("POST", "apps", prepared.uid, mutationKey("$owner:create"), body)
                .getJSONObject("data").getString("id")
            save("$owner:appId", remote)
        }
        check(remote.matches(Regex("[A-Za-z0-9_-]{1,128}")))
        var version = preferences.getString("$release:versionId", null)
        if (version != null) {
            val state = write("GET", "versions/$version", prepared.uid, "").getJSONObject("data")
            if (state.optString("status") == "failed" && state.optJSONObject("error")?.optString("code") == "UPLOAD_EXPIRED") {
                val attempt = UUID.randomUUID().toString()
                // This user retry follows a confirmed terminal expiry, rather
                // than an unknown network result. Previous receipts are kept.
                save("$publication:attempt", attempt)
                release = "$publication:$attempt"
                version = null
            }
        }
        if (version == null) {
            val body = preferences.getString("$release:versionBody", null)?.let(::JSONObject) ?: JSONObject()
                .put("format", "zip").put("filename", "source.zip").put("sizeBytes", prepared.archive.length())
                .put("sha256", prepared.sha256).put("notes", "Local release ${prepared.revision}")
                .also { save("$release:versionBody", it.toString()) }
            version = write("POST", "apps/$remote/versions", prepared.uid, mutationKey("$release:createVersion"), body)
                .getJSONObject("data").getString("id")
            save("$release:versionId", version)
        }
        check(version.matches(Regex("[A-Za-z0-9_-]{1,128}")))
        write("PUT", "versions/$version/artifact", prepared.uid, mutationKey("$release:upload"), file = prepared.archive)
        val result = write("POST", "versions/$version/publish", prepared.uid, mutationKey("$release:publish"), JSONObject())
        val status = result.getJSONObject("data").getString("status")
        save("$release:status", status)
        status
    } }

    private suspend fun write(method: String, path: String, uid: String, idempotencyKey: String, json: JSONObject? = null, file: File? = null): JSONObject {
        checkAccount(uid)
        val token = auth.currentUser!!.getIdToken(false).await().token ?: throw MarketException("SIGN_IN_REQUIRED")
        checkAccount(uid)
        val url = URL(market.apiRoot.trimEnd('/') + "/" + path)
        check(url.protocol == "https" && url.userInfo == null)
        val request = (url.openConnection() as HttpURLConnection).apply {
            instanceFollowRedirects = false; connectTimeout = 15_000; readTimeout = 60_000
            requestMethod = method; doOutput = method != "GET"
            setRequestProperty("Authorization", "Bearer $token")
            if (method != "GET") setRequestProperty("Idempotency-Key", idempotencyKey)
            setRequestProperty("Content-Type", if (file != null) "application/octet-stream" else "application/json")
        }
        try {
            if (file != null) {
                request.setFixedLengthStreamingMode(file.length())
                request.outputStream.use { output -> file.inputStream().use { input ->
                    val buffer = ByteArray(32768)
                    while (true) { coroutineContext.ensureActive(); val count = input.read(buffer); if (count < 0) break; output.write(buffer, 0, count) }
                } }
            } else if (method != "GET") request.outputStream.use { it.write((json ?: JSONObject()).toString().toByteArray()) }
            val code = request.responseCode
            val input = if (code in 200..299) request.inputStream else request.errorStream
            val bytes = input?.use { stream ->
                val output = java.io.ByteArrayOutputStream(); val buffer = ByteArray(8192)
                while (true) { coroutineContext.ensureActive(); val count = stream.read(buffer); if (count < 0) break; check(output.size() + count <= 2 * 1024 * 1024) { "Market response too large" }; output.write(buffer, 0, count) }
                output.toByteArray()
            }
            val result = runCatching { JSONObject(bytes?.decodeToString().orEmpty()) }.getOrElse { throw MarketException("HTTP_$code") }
            if (code !in 200..299) {
                val error = result.optJSONObject("error")
                throw MarketException(error?.optString("code") ?: "HTTP_$code", error?.optString("message") ?: "HTTP_$code")
            }
            checkAccount(uid)
            return result
        } finally { request.disconnect() }
    }
    companion object {
        private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        private fun sha(file: File): String {
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input -> val bytes = ByteArray(32768); while (true) { val count = input.read(bytes); if (count < 0) break; digest.update(bytes, 0, count) } }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }
    }
}
