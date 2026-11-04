package com.lingxi.code.theme

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Both tables come from the real Desktop module (`AgentAvatar.tsx`): the 34-entry
 * `agent-avatar-identities.json` shared with iOS and the `avatars.indexPrefixed`
 * rows of `mobile-team-bot-vectors.json` (SPEC-mobile-team-bot-ui section 7.2 / 8.5).
 * Both files reach the class path through the `packages/bridge-client/fixtures`
 * test-resources directory declared in `app/build.gradle.kts`.
 */
class AgentAvatarTest {
    private fun fixtureText(name: String): String {
        val stream = requireNotNull(javaClass.classLoader?.getResourceAsStream(name)) {
            "$name is not on the unit-test class path (packages/bridge-client/fixtures)"
        }
        return stream.bufferedReader(Charsets.UTF_8).use { it.readText() }
    }

    private fun JSONArray.objects(): List<JSONObject> = (0 until length()).map { getJSONObject(it) }

    @Test fun sharedIdentityFixtureMatchesTheDesktopRule() {
        val entries = JSONArray(fixtureText("agent-avatar-identities.json")).objects()
        assertEquals(34, entries.size)
        entries.forEach { entry ->
            val id = entry.getString("id")
            assertEquals("agentAvatarIndex(\"$id\")", entry.getInt("index"), agentAvatarIndex(id))
        }
    }

    @Test fun specVectorsIncludingTheAgentPrefixedForm() {
        val vectors = JSONObject(fixtureText("mobile-team-bot-vectors.json"))
            .getJSONObject("avatars").getJSONArray("indexPrefixed").objects()
        assertEquals(8, vectors.size)
        vectors.forEach { vector ->
            // The file stores the 128 x "a" case as "a×128" plus a repeat descriptor.
            val id = vector.optJSONObject("repeat")
                ?.let { it.getString("char").repeat(it.getInt("count")) }
                ?: vector.getString("agentId")
            assertEquals(vector.getString("agentId"), vector.getInt("expect"), agentAvatarIndex(id))
        }
    }

    @Test fun desktopUtf16IdentityVectors() {
        listOf("" to 14, "main" to 1, "agent-review-42" to 26, "搜索-agent" to 2, "🧠-review" to 26, "a".repeat(128) to 12)
            .forEach { (id, expected) -> assertEquals(id, expected, agentAvatarIndex(id)) }
    }

    @Test fun bareIdAndAgentPrefixedIdDrawTheSameFace() {
        val uuid = "c0ffee00-1234-5678-9abc-def012345678"
        assertEquals(14, agentAvatarIndex(uuid))
        assertEquals(agentAvatarIndex(uuid), agentAvatarIndex("agent:$uuid"))
        // The prefix is added once, never doubled: "agent-0" and "agent:agent-0" are the same key.
        assertEquals(agentAvatarIndex("agent-0"), agentAvatarIndex("agent:agent-0"))
    }

    @Test fun everyOneOfTheTwentyEightFacesIsReachable() {
        assertEquals(28, (0..299).map { agentAvatarIndex("agent-$it") }.toSet().size)
    }
}
