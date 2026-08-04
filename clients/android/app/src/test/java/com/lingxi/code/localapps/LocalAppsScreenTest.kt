package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppsScreenTest {
    /** The literal rule from lingxi-code/local-apps/src/manifest.rs validate_identifier. */
    private val identifier = Regex("^[a-z][a-z0-9_]{0,63}$")

    @Test
    fun generatedFieldIdMatchesTheEngineIdentifierContract() {
        val first = generateFieldId(emptyList())
        assertTrue("minted id $first is rejected by the manifest", identifier.matches(first))

        val second = generateFieldId(listOf(field(first)))
        assertTrue("minted id $second is rejected by the manifest", identifier.matches(second))
        assertEquals("field_2", second)

        // The natural index is already taken, so the loop must step past it.
        assertEquals("field_3", generateFieldId(listOf(field("field_2"))))
    }

    @Test
    fun `domain editor rejects everything the manifest contract rejects`() {
        // Both enable the Add button under Kotlin's Unicode-aware
        // isLetterOrDigit(), and both are refused by manifest::validate_domain
        // at ingest, so the designer could offer a value it can never persist.
        assertFalse("uppercase is not ASCII-lowercase", isValidDomain("API.Example.com"))
        assertFalse("non-ASCII labels are rejected by the engine", isValidDomain("北京.cn"))

        assertTrue(isValidDomain("api.example.com"))
        assertTrue(isValidDomain("cdn-1.example.com"))
        assertFalse(isValidDomain("https://api.example.com"))
        assertFalse(isValidDomain("-api.example.com"))
        assertFalse(isValidDomain("api..example.com"))
    }

    private fun field(id: String) = LocalAppDataField(id, "新字段", LocalAppDataFieldType.Text, false)
}
