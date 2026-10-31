package com.lingxi.code.apps

import android.content.Context
import com.lingxi.code.BuildConfig
import com.lingxi.code.market.MarketClient
import com.lingxi.code.market.MarketDistribution
import com.lingxi.code.market.MarketException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

/** v2 ships source packages into the same local installation store as authored apps. */
class UnifiedAppMarket(private val context: Context, private val apps: UnifiedLocalApps, apiRoot: String = BuildConfig.LOCAL_APP_MARKET_API_ROOT) : MarketClient(apiRoot) {
    private val installMutex = Mutex()
    val publisher = AppMarketPublisher(context, apps, this)
    private fun target(environment: String) = appTarget(environment)
    suspend fun capabilities(): JSONObject = request("/capabilities").getJSONObject("data").also { supported ->
        require(supported.getString("apiVersion") == "v2" && supported.getInt("packageSchema") == 2 &&
            supported.getJSONObject("features").getBoolean("targetSelection")) { "Market does not support compatible schema-2 source packages" }
    }
    val browseUrl: String get() = androidMarketBrowseUrl(apiRoot)
    suspend fun distribution(remoteId: String): JSONObject {
        requireMarketId(remoteId)
        val candidates = mutableListOf<JSONObject>()
        for (environment in if (BuildConfig.MOBILE_LINUX_FULL) listOf("web", "mobile-linux") else listOf("web")) {
            try {
                val value = request("/apps/$remoteId/distribution", JSONObject().put("target", target(environment))).getJSONObject("data")
                requireAndroidMarketDistribution(value, remoteId, setOf(environment))
                candidates.add(value)
            } catch (error: MarketException) { if (error.code != "NO_COMPATIBLE_RELEASE") throw error }
        }
        return candidates.maxByOrNull { it.getLong("sequence") } ?: throw MarketException("NO_COMPATIBLE_RELEASE")
    }
    suspend fun install(remoteId: String, existing: AppRecord? = null, selected: JSONObject? = null): AppRecord = installMutex.withLock {
        withContext(Dispatchers.IO) {
            require(existing == null || existing.marketOrigin?.let { it.appId == remoteId && it.remixedFrom == null } == true) { "Author updates cannot replace a customized application" }
            apps.refresh()
            val installation = existing ?: apps.state.value.apps.firstOrNull { it.marketOrigin?.let { origin -> origin.appId == remoteId && origin.remixedFrom == null } == true }
            val distribution = selected ?: distribution(remoteId)
            requireAndroidMarketDistribution(distribution, remoteId, if (BuildConfig.MOBILE_LINUX_FULL) setOf("web", "mobile-linux") else setOf("web"))
            if (installation?.marketOrigin?.releaseId == distribution.getString("versionId")) return@withContext installation
            if (installation != null && apps.release(installation.id, installation.latestRevision).marketOrigin?.releaseId == distribution.getString("versionId")) return@withContext installation
            val artifact = distribution.getJSONObject("artifact")
            val stage = File(context.cacheDir, "local-app-v2-import/${UUID.randomUUID()}")
            check(stage.mkdirs()) { "Unable to create package staging directory" }
            try {
                val archive = File(stage, "package.zip")
                download(MarketDistribution(remoteId, distribution.getString("versionId"), distribution.getLong("sequence"),
                    artifact.getString("format"), artifact.getLong("sizeBytes"), artifact.getString("sha256"), artifact.getString("downloadUrl")), archive) { _, _ -> }
                val source = File(stage, "source")
                check(source.mkdir())
                unpackSource(archive, source, distribution.getJSONArray("files"))
                val manifest = JSONObject(File(source, "lingxi-app.json").readText())
                require(manifest.toString() == distribution.getJSONObject("manifest").toString() ||
                    canonicalJson(manifest) == canonicalJson(distribution.getJSONObject("manifest"))) { "Package manifest differs from its release" }
                val origin = JSONObject().put("appId", remoteId).put("releaseId", distribution.getString("versionId")).put("digest", distribution.getString("releaseDigest"))
                if (installation == null) apps.importSource(source.absolutePath, origin) else {
                    val result = apps.request("publish", installation.id) {
                        put("sourcePath", source.absolutePath); put("expectedRevision", installation.latestRevision); put("marketOrigin", origin)
                    } as JSONObject
                    apps.refresh()
                    val updated = result.getJSONObject("record").appRecord()
                    val release = result.getJSONObject("release").appRelease()
                    // New executable content stays pending until its digest is approved.
                    if (!release.executesCode || release.digest in updated.executionGrants) apps.activate(updated.id, release.revision)
                    apps.get(updated.id)
                }
            } finally { stage.deleteRecursively() }
        }
    }
    companion object {
        internal fun canonicalJson(value: Any?): String = when (value) {
            is JSONObject -> value.keys().asSequence().toList().sorted().joinToString(",", "{", "}") { JSONObject.quote(it) + ":" + canonicalJson(value.get(it)) }
            is JSONArray -> (0 until value.length()).joinToString(",", "[", "]") { canonicalJson(value.get(it)) }
            is String -> JSONObject.quote(value)
            else -> value.toString()
        }
        internal suspend fun unpackSource(archive: File, output: File, inventory: JSONArray) = unpackAppSource(archive, output, inventory)
    }
}
