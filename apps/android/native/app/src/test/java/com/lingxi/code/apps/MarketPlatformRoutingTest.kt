package com.lingxi.code.apps

import com.lingxi.code.BuildConfig
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class MarketPlatformRoutingTest {
    private fun distribution(platform: String, environment: String = "web", channel: String? = null): JSONObject {
        val target = JSONObject().put("platform", platform).put("environment", environment)
        if (channel != null) target.put("channels", org.json.JSONArray(listOf(channel)))
        return JSONObject().put("schema", 2).put("appId", "market-id")
            .put("manifest", JSONObject().put("targets", org.json.JSONArray().put(target)))
    }
    @Test fun `market browser opens Android filter without leaking API path`() {
        assertEquals("https://market.example/?platform=android", androidMarketBrowseUrl("https://market.example/api/v2"))
    }
    @Test fun `only current platform and allowed environment can install`() {
        requireAndroidMarketDistribution(distribution("android"), "market-id", setOf("web"))
        for (platform in listOf("ios", "macos", "windows", "linux")) {
            assertThrows(IllegalArgumentException::class.java) {
                requireAndroidMarketDistribution(distribution(platform), "market-id", setOf("web"))
            }
        }
        assertThrows(IllegalArgumentException::class.java) {
            requireAndroidMarketDistribution(distribution("android", "mobile-linux"), "market-id", setOf("web"))
        }
    }
    @Test fun `distribution channel and identity cannot be substituted`() {
        assertTrue(BuildConfig.DISTRIBUTION_CHANNEL in setOf("store", "full"))
        requireAndroidMarketDistribution(distribution("android", channel = BuildConfig.DISTRIBUTION_CHANNEL), "market-id", setOf("web"))
        assertThrows(IllegalArgumentException::class.java) {
            requireAndroidMarketDistribution(distribution("android", channel = "other"), "market-id", setOf("web"))
        }
        for (id in listOf("../app", "a/b", "https://evil.test/package.zip")) {
            assertThrows(IllegalArgumentException::class.java) { requireMarketId(id) }
        }
        assertThrows(IllegalArgumentException::class.java) {
            requireAndroidMarketDistribution(distribution("android"), "other-id", setOf("web"))
        }
    }
}
