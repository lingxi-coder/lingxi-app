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

    @Test fun retainedOwnerKeepsItsAccountWhenAnotherEngineBecomesVisible() {
        val routes = OwnerScopedProviderAudioBindings<Any>()
        val first = Any()
        val second = Any()
        val ownerA = AudioOwnerKey.session("session")
        val ownerB = AudioOwnerKey.session("other")
        routes.bind(ownerA, first)
        routes.bind(ownerB, second)
        val initial = context()
        routes.context(ownerA, first, initial)
        routes.record(ownerA, first, initial, "account.B", "scope-B")
        routes.attach(second)
        assertEquals("scope-B", routes.context(ownerA, first, context())?.getString("accountScope"))
        assertEquals("unbound", routes.context(ownerB, second, context(session = "other"))?.getString("accountScope"))
    }

    @Test fun replacingAnOwnerEngineRejectsLateAccountResults() {
        data class Engine(val name: String)
        val routes = OwnerScopedProviderAudioBindings<Engine>()
        val owner = AudioOwnerKey.session("session")
        val first = Engine("equal")
        val replacement = Engine("equal")
        routes.bind(owner, first)
        routes.context(owner, first, context())
        routes.record(owner, first, context(), "old", "old-scope")
        routes.bind(owner, replacement)
        routes.record(owner, first, context(), "late", "late-scope")
        assertNull(routes.context(owner, first, context()))
        assertEquals("unbound", routes.context(owner, replacement, context())?.getString("accountScope"))
        routes.releaseIfBound(owner, first)
        assertSame(replacement, routes.engineFor(owner))
        routes.detach(replacement)
        assertNull(routes.engineFor(owner))
    }
}
