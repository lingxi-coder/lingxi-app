package com.lingxi.code.navigation
import org.junit.Assert.*
import org.junit.Test
class DeepLinkTest {
    @Test fun messageUuidIsNormalizedWithoutChangingCaseSensitiveResourceIdentity() {
        val link = DeepLink.parse("lingxi://session/AbC?message=123E4567-E89B-12D3-A456-426614174000")!!
        assertEquals("session/AbC", link.route)
        assertEquals("123e4567-e89b-12d3-a456-426614174000", link.params["message"])
    }
    @Test fun canonicalDraftPreservesUnicodeAndReservedCharacters() {
        val link = DeepLink.parse("https://lingxios.net/open/new#payload=%7B%22prompt%22%3A%22hello+world%26next%22%7D")!!
        assertEquals("hello world&next", link.payload["prompt"])
    }
    @Test fun rejectsAmbiguousLinks() {
        for (raw in listOf("https://lingxios.net.evil/open/new", "lingxi://new?mode=chat&mode=code", "lingxi://session/%2fsecret", "lingxi://new?unknown=x", "lingxi://user:password@new")) assertNull(raw, DeepLink.parse(raw))
    }
}

class SharedDeepLinkFixtureTest {
    @Test fun sharedCrossLanguageCorpus() {
        val root = generateSequence(java.io.File(checkNotNull(System.getProperty("user.dir")))) { it.parentFile }
            .map { java.io.File(it, "packages/deep-links/fixtures.json") }.first { it.isFile }
        val corpus = org.json.JSONObject(root.readText())
        val valid = corpus.getJSONArray("valid")
        for (i in 0 until valid.length()) {
            val fixture = valid.getJSONObject(i)
            val link = DeepLink.parse(fixture.getString("url"))
            assertNotNull(fixture.toString(), link)
            assertEquals(fixture.getString("route"), link!!.route)
            fixture.optJSONObject("params")?.let { expected ->
                expected.keys().forEach { key -> assertEquals(expected.getString(key), link.params[key]) }
            }
        }
        val invalid = corpus.getJSONArray("invalid")
        for (i in 0 until invalid.length()) assertNull(invalid.getString(i), DeepLink.parse(invalid.getString(i)))
    }
    @Test fun rejectsCredentialBearingNestedImportsAndAllowsReviewData() {
        assertNull(DeepLink.parse("lingxi://import/mcp#payload=%7B%22config%22%3A%7B%22env%22%3A%7B%22KEY%22%3A%22secret%22%7D%7D%7D"))
        val payload = mapOf("name" to "test", "config" to org.json.JSONObject().put("url", "https://example.test/mcp"))
        assertNotNull(DeepLink.parse(DeepLink.build("import/mcp", payload = payload)))
    }
}
