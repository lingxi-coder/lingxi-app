package com.lingxi.code.settings

import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderConnection
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * One provider reached several ways: the editing rules, the validation rules,
 * and what those turn into in the provider settings the engine is launched with.
 *
 * The engine-side desugaring is covered by `connection_group_test.rs`; what is
 * proved here is that the Android store can express it, and that a provider
 * carrying connections survives the emit path instead of being skipped as a
 * built-in.
 */
class ProviderConnectionsTest {

    // ── Editing rules ────────────────────────────────────────────────────────

    @Test
    fun firstAdd_migratesTheFlatProviderIntoTwoConnections() {
        val next = ProviderConnections.withAddedConnection(provider())

        assertEquals(2, next.connections.size)
        assertEquals("default", next.connections[0].id)
        assertEquals("https://api.deepseek.com", next.connections[0].url)
        assertEquals("", next.connections[1].id)
        assertEquals("", next.connections[1].url)
    }

    @Test
    fun furtherAdds_appendOneEmptyConnection() {
        val next = ProviderConnections.withAddedConnection(
            ProviderConnections.withAddedConnection(provider()),
        )

        assertEquals(3, next.connections.size)
        assertEquals("default", next.connections[0].id)
    }

    @Test
    fun removingBackToOne_collapsesAndKeepsTheSurvivingEndpoint() {
        val two = ProviderConnections.withUpdatedConnection(
            ProviderConnections.withAddedConnection(provider()),
            1,
        ) { it.copy(id = "cn", url = "https://api.deepseek.cn/v1") }

        val collapsed = ProviderConnections.withRemovedConnection(two, 0)

        assertTrue(collapsed.connections.isEmpty())
        assertEquals("https://api.deepseek.cn/v1", collapsed.url)
    }

    @Test
    fun removingAnOutOfRangeIndex_changesNothing() {
        val two = ProviderConnections.withAddedConnection(provider())

        assertEquals(two, ProviderConnections.withRemovedConnection(two, 7))
    }

    // ── Validation ───────────────────────────────────────────────────────────

    @Test
    fun aProviderWithoutConnections_validates() {
        assertNull(ProviderConnections.validate(provider()))
    }

    @Test
    fun anEmptyConnectionId_isRejected() {
        val subject = withSecondConnection(id = "", url = "https://api.deepseek.cn/v1")

        assertEquals(ProviderConnectionProblem.MissingId, ProviderConnections.validate(subject))
    }

    /**
     * `:` would produce `group:a:b`, which the engine splits at the wrong colon;
     * `/` would break the `profile/model` qualified reference.
     */
    @Test
    fun connectionIds_cannotCarryTheEnginesSeparators() {
        for (bad in listOf("cn:1", "cn/1")) {
            val subject = withSecondConnection(id = bad, url = "https://api.deepseek.cn/v1")

            assertEquals(
                "$bad must be rejected",
                ProviderConnectionProblem.MissingId,
                ProviderConnections.validate(subject),
            )
        }
    }

    @Test
    fun duplicateConnectionIds_areRejected() {
        val subject = withSecondConnection(id = "default", url = "https://api.deepseek.cn/v1")

        assertEquals(
            ProviderConnectionProblem.DuplicateId("default"),
            ProviderConnections.validate(subject),
        )
    }

    /**
     * A connection with no endpoint of its own would inherit the provider's —
     * i.e. point at the endpoint the user is adding one BESIDE.
     */
    @Test
    fun aConnectionMustCarryItsOwnEndpoint() {
        val subject = withSecondConnection(id = "cn", url = "")

        assertEquals(
            ProviderConnectionProblem.InvalidUrl("cn"),
            ProviderConnections.validate(subject),
        )
    }

    @Test
    fun aNonHttpConnectionEndpoint_isRejected() {
        val subject = withSecondConnection(id = "cn", url = "ftp://api.deepseek.cn")

        assertEquals(
            ProviderConnectionProblem.InvalidUrl("cn"),
            ProviderConnections.validate(subject),
        )
    }

    // ── Persistence ──────────────────────────────────────────────────────────

    @Test
    fun providerSavedBeforeConnectionsExisted_decodesAsReachableOneWay() {
        val legacy = """
            [{"id":"l_deepseek","preset":"deepseek","name":"DeepSeek",
              "url":"https://api.deepseek.com","model":"deepseek-flash","enabled":true}]
        """.trimIndent()

        val decoded = ProviderSettingsRepository.decodeProvidersJson(legacy)

        assertEquals(1, decoded.size)
        assertTrue(decoded[0].connections.isEmpty())
    }

    @Test
    fun connectionsSurviveAnEncodeDecodeRoundTrip() {
        val subject = withSecondConnection(
            id = "cn",
            url = "https://api.deepseek.cn/v1",
            modelIds = listOf("deepseek-flash"),
        )

        val decoded = ProviderSettingsRepository.decodeProvidersJson(
            ProviderSettingsRepository.encodeProvidersJson(listOf(subject)),
        )

        assertEquals(listOf("default", "cn"), decoded[0].connections.map { it.id })
        assertEquals(listOf("deepseek-flash"), decoded[0].connections[1].modelIds)
    }

    // ── What the engine is launched with ─────────────────────────────────────

    @Test
    fun twoConnections_reachTheEmittedProviderSettings() {
        val subject = withSecondConnection(
            id = "cn",
            url = "https://api.deepseek.cn/v1",
            modelIds = listOf("deepseek-flash"),
        )

        val profiles = JSONObject(
            ProviderSettingsRepository.buildEngineLaunchConfig(listOf(subject)).providerProfilesJson,
        )
        val entry = profiles.getJSONObject(profiles.keys().next())
        val connections = entry.getJSONArray("connections")

        assertEquals(2, connections.length())
        assertEquals("default", connections.getJSONObject(0).getString("id"))
        assertEquals("https://api.deepseek.com", connections.getJSONObject(0).getString("baseUrl"))
        assertTrue(
            "a connection serving every model must not pin a subset",
            connections.getJSONObject(0).optJSONArray("models") == null,
        )
        assertEquals("cn", connections.getJSONObject(1).getString("id"))
        assertEquals(
            "deepseek-flash",
            connections.getJSONObject(1).getJSONArray("models").getJSONObject(0).getString("id"),
        )
    }

    /**
     * The regression this feature is one line away from: a provider sitting on
     * the catalog's official endpoint is deliberately SKIPPED from the emitted
     * map. Adding connections must take it out of that class — otherwise they
     * are dropped with no error on any surface and the picker still looks right.
     */
    @Test
    fun officialEndpointProviderWithConnections_isStillEmitted() {
        val flat = provider()
        val before = JSONObject(
            ProviderSettingsRepository.buildEngineLaunchConfig(listOf(flat)).providerProfilesJson,
        )
        assertEquals(
            "precondition: the official endpoint is emitted as a built-in",
            0,
            before.length(),
        )

        val withConnections = withSecondConnection(id = "cn", url = "https://api.deepseek.cn/v1")
        val after = JSONObject(
            ProviderSettingsRepository.buildEngineLaunchConfig(listOf(withConnections)).providerProfilesJson,
        )

        assertEquals(1, after.length())
        assertEquals(
            2,
            after.getJSONObject(after.keys().next()).getJSONArray("connections").length(),
        )
    }

    private fun provider() = GenericProvider(
        id = "l_deepseek",
        preset = "deepseek",
        name = "DeepSeek",
        url = "https://api.deepseek.com",
        key = "",
        model = "deepseek-flash",
        status = ConnStatus.Idle,
        enabled = true,
        credentialConfigured = true,
    )

    private fun withSecondConnection(
        id: String,
        url: String,
        modelIds: List<String>? = null,
    ): GenericProvider = provider().copy(
        connections = listOf(
            ProviderConnection(id = "default", url = "https://api.deepseek.com"),
            ProviderConnection(id = id, url = url, modelIds = modelIds),
        ),
    )
}
