package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class PermissionModeSettingsJsonTest {
    @get:Rule val temporary = TemporaryFolder()

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

    @Test
    fun legacySettingsImportIsSameBrandOnlyAndKeepsOriginalFiles() {
        val appFiles = temporary.newFolder("app-files")
        val lingxiLegacy = permissionModeLegacySettingsDirectory(appFiles, "lingxi", "lingxi", ".lingxi")
        assertEquals(File(appFiles, ".lingxi"), lingxiLegacy)
        assertNull(permissionModeLegacySettingsDirectory(appFiles, "atlas", "atlas_guest", ".atlas"))
        assertNull(permissionModeLegacySettingsDirectory(appFiles, "lingxi", "atlas_guest", ".lingxi"))

        val source = checkNotNull(lingxiLegacy).apply { mkdirs() }
        File(source, "settings.json").writeText("{\"permissions\":{\"defaultMode\":\"plan\"}}")
        File(source, "last-permission-mode.json").writeText("{\"mode\":\"acceptEdits\"}")
        val target = File(temporary.root, "persistent/lingxi/.lingxi")
        copyPermissionSettingsIfMissing(source, target)

        assertEquals(File(source, "settings.json").readText(), File(target, "settings.json").readText())
        assertEquals(File(source, "last-permission-mode.json").readText(), File(target, "last-permission-mode.json").readText())
        assertTrue(File(source, "settings.json").isFile)

        File(target, "settings.json").writeText("{\"permissions\":{\"defaultMode\":\"default\"}}")
        copyPermissionSettingsIfMissing(source, target)
        assertEquals("default", JSONObject(File(target, "settings.json").readText()).getJSONObject("permissions").getString("defaultMode"))
    }
}
