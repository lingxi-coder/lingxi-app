package com.lingxi.code.apps

import com.lingxi.code.BuildConfig
import org.junit.Assert.*
import org.junit.Test

class BotMarketApiRootTest {
    @Test fun `Bot root uses explicit canonical v3 endpoint and leaves browser origin unchanged`() {
        assertEquals("https://marketgateway-pacws5zsoa-uc.a.run.app/api/v3/", requireBotMarketApiRoot(BuildConfig.BOT_MARKET_API_ROOT))
        val handoff = BotMarketHandoff("travel-helper")
        assertEquals("https://lingxi-5e4b2-market.web.app/bots/travel-helper?platform=macos", handoff.marketUrl)
    }

    @Test fun `Bot root rejects credentials alternate paths queries and fragments`() {
        assertEquals("https://fixture.test/api/v3/", requireBotMarketApiRoot("https://fixture.test/api/v3/"))
        for (root in listOf("http://fixture.test/api/v3/", "https://user:secret@fixture.test/api/v3/",
            "https://fixture.test/api/v2/", "https://fixture.test/api/v3", "https://fixture.test/api/v3/extra",
            "https://fixture.test/api/v3/?x=1", "https://fixture.test/api/v3/#part", "https://fixture.test/api/%763/")) {
            assertThrows(root, IllegalArgumentException::class.java) { requireBotMarketApiRoot(root) }
        }
    }
}
