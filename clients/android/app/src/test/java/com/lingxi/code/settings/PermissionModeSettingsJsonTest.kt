package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PermissionModeSettingsJsonTest {
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
