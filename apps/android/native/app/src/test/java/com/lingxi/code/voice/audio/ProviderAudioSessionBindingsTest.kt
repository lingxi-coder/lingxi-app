package com.lingxi.code.voice.audio

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ProviderAudioSessionBindingsTest {
    private fun context(session: String = "session", profile: String = "openai") =
        JSONObject().put("sessionId", session).put("profileId", profile).put("accountScope", "unbound")

    @Test fun actualAccountBRetainsItsResourcesAcrossRepeatedUnboundSnapshots() {
        val routes = ProviderAudioSessionBindings<String>()
        val initial = context()
        routes.context("owner", initial)
        routes.record("owner", initial, "account.B", "{\"accountId\":\"B\"}")
        repeat(3) {
            val actual = routes.context("owner", context())
            assertEquals("account.B", actual.getString("profileId"))
            assertEquals("{\"accountId\":\"B\"}", actual.getString("accountScope"))
        }
        assertEquals("unbound", initial.getString("accountScope"))
    }

    @Test fun anotherOwnerDoesNotInheritAccountB() {
        val routes = ProviderAudioSessionBindings<String>()
        routes.context("owner", context())
        routes.record("owner", context(), "account.B", "scope-B")
        assertEquals("unbound", routes.context("another", context()).getString("accountScope"))
    }

    @Test fun newSessionOrExplicitProfileRebindClearsOldScopeAndRejectsLateOldReply() {
        for (replacement in listOf(context(session = "new-session"), context(profile = "gemini"))) {
            val routes = ProviderAudioSessionBindings<String>()
            routes.context("owner", context())
            routes.record("owner", context(), "account.B", "scope-B")
            assertEquals("unbound", routes.context("owner", replacement).getString("accountScope"))
            routes.record("owner", context(), "account.B", "late-scope")
            assertEquals("unbound", routes.context("owner", replacement).getString("accountScope"))
        }
    }

    @Test fun clearingEngineBindingCannotReviveOldResources() {
        val routes = ProviderAudioSessionBindings<String>()
        routes.context("owner", context())
        routes.record("owner", context(), "account.B", "scope-B")
        routes.clear()
        routes.record("owner", context(), "account.B", "late-scope")
        assertEquals("unbound", routes.context("owner", context()).getString("accountScope"))
    }
}
