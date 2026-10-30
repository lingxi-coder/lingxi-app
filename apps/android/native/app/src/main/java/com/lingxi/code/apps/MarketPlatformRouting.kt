package com.lingxi.code.apps

import com.lingxi.code.BuildConfig
import org.json.JSONObject

internal fun requireMarketId(value: String) {
    require(Regex("^[A-Za-z0-9_-]{1,128}$").matches(value)) { "Invalid market application ID" }
}

internal fun requireAndroidMarketDistribution(value: JSONObject, remoteId: String, environments: Set<String>) {
    requireMarketId(remoteId)
    require(value.getInt("schema") == 2 && value.getString("appId") == remoteId) { "Invalid market distribution" }
    val targets = value.getJSONObject("manifest").getJSONArray("targets")
    require((0 until targets.length()).any { index ->
        val target = targets.getJSONObject(index)
        target.getString("platform") == "android" && target.getString("environment") in environments &&
            (target.optJSONArray("channels")?.let { channels -> channels.length() == 0 || (0 until channels.length()).any { channels.getString(it) == BuildConfig.DISTRIBUTION_CHANNEL } } ?: true)
    }) { "This release does not support Android on this distribution channel" }
}

internal fun androidMarketBrowseUrl(apiRoot: String): String = java.net.URI(apiRoot).resolve("/?platform=android").toString()
