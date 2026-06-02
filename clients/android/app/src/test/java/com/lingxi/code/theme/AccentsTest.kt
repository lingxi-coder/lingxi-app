package com.lingxi.code.theme

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Integrity checks for the 6-swatch Appearance accent picker — counts, the
 * canonical oklch id strings (which double as the persisted selection key), the
 * converted sRGB swatch colors, and the [Accents.color] lookup + fallback. The
 * id list and order are copied verbatim from the iOS `Accents.all`.
 */
class AccentsTest {

    /** The exact id (oklch string) + name + sRGB color, in prototype order. */
    private val expected = listOf(
        Triple("oklch(70% 0.18 268)", "靛紫", Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f)),
        Triple("oklch(70% 0.18 320)", "玫红", Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f)),
        Triple("oklch(72% 0.16 195)", "青蓝", Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f)),
        Triple("oklch(72% 0.16 155)", "青绿", Color(red = 0.2085f, green = 0.7571f, blue = 0.4656f)),
        Triple("oklch(74% 0.16 75)", "琥珀", Color(red = 0.8960f, green = 0.6013f, blue = 0.0000f)),
        Triple("oklch(70% 0.20 30)", "砖红", Color(red = 1.0000f, green = 0.3802f, blue = 0.3010f)),
    )

    @Test
    fun sixAccents_inExactOrder_withCanonicalIdsNamesAndColors() {
        assertEquals("accent count", 6, Accents.all.size)
        Accents.all.forEachIndexed { i, opt ->
            val (id, name, color) = expected[i]
            assertEquals("accent[$i].id", id, opt.id)
            assertEquals("accent[$i].name", name, opt.name)
            assertEquals("accent[$i].color", color, opt.color)
        }
    }

    @Test
    fun accentIds_areUnique() {
        val ids = Accents.all.map { it.id }
        assertEquals("ids are unique", ids.size, ids.toSet().size)
    }

    @Test
    fun defaultId_isFirstSwatch_andMatchesDarkAccent() {
        assertEquals("DEFAULT_ID is 靛紫", "oklch(70% 0.18 268)", Accents.DEFAULT_ID)
        assertEquals("DEFAULT_ID == all[0].id", Accents.all[0].id, Accents.DEFAULT_ID)
        // The default accent color is the dark/light palette accent (268 hue).
        assertEquals(
            "DEFAULT_ID color == dark.accent",
            DesignTokens.dark.accent,
            Accents.color(Accents.DEFAULT_ID),
        )
    }

    @Test
    fun colorLookup_resolvesById() {
        assertEquals(Accents.all[2].color, Accents.color("oklch(72% 0.16 195)"))
        assertEquals(Accents.all[5].color, Accents.color("oklch(70% 0.20 30)"))
    }

    @Test
    fun colorLookup_fallsBackToFirst_forUnknownId() {
        // Color is a value class, so identical colors are equal-by-value (not
        // the same reference) — assert value equality.
        assertEquals(Accents.all[0].color, Accents.color("oklch(does-not-exist)"))
        assertEquals(Accents.all[0].color, Accents.color(""))
    }
}
