package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PermissionModeSettingsJsonTest {
    @Test
    fun engineSelectionTakesPrecedenceOverLegacyDefault() {
        assertEquals("plan", PermissionModeSettingsJson.load(
            """{"permissions":{"defaultMode":"auto"}}""",
            """{"mode":"plan"}""",
        ))
        assertEquals("bypassPermissions", PermissionModeSettingsJson.load(
            null,
            """{"mode":"bypassPermissions"}""",
        ))
    }

    @Test
    fun malformedOrUnknownSelectionFallsBackToLegacyDefault() {
        val legacy = """{"permissions":{"defaultMode":"acceptEdits"}}"""
        assertEquals("acceptEdits", PermissionModeSettingsJson.load(legacy, "broken"))
        assertEquals("acceptEdits", PermissionModeSettingsJson.load(legacy, """{"mode":"unknown"}"""))
    }

    @Test
    fun missingOrInvalidModeFallsBackToAuto() {
        assertEquals("auto", PermissionModeSettingsJson.load(null))
        assertEquals("auto", PermissionModeSettingsJson.load("{\"permissions\":{\"defaultMode\":\"auto-model\"}}"))
    }

    @Test
    fun updatePreservesUnknownTopLevelAndPermissionFields() {
        val updated = JSONObject(
            PermissionModeSettingsJson.update(
                """{"unknown":true,"permissions":{"custom":42}}""",
                "acceptEdits",
            ),
        )
        assertTrue(updated.optBoolean("unknown"))
        assertEquals(42, updated.getJSONObject("permissions").getInt("custom"))
        assertEquals("acceptEdits", updated.getJSONObject("permissions").getString("defaultMode"))
    }
}
