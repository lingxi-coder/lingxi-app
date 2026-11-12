package com.lingxi.code.theme

import androidx.compose.material3.ColorScheme
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.luminance
import com.lingxi.code.teams.OWN_MESSAGE_TINT
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Locale

/**
 * Every Material colour role of the app's scheme (docs/TEAMS-UI-POLISH.md Appendix D, Android rows), for each of the six accents in light and dark:
 * the ink on a tone and on its container reads at 4.5:1, the ink on every surface rung reads at 4.5:1, the accent, error and ok tones are legible as
 * text and as graphics, and the surface ladder runs the right way. The worst case of each pair is printed, so a run of this class is the source of the
 * numbers in the report.
 */
class ThemeColorRolesTest {
    private class Case(val dark: Boolean, val accent: String) {
        val palette = resolvePalette(dark, accent)
        val scheme: ColorScheme = palette.toColorScheme()
        override fun toString() = "${if (dark) "dark" else "light"} $accent"
    }

    private val cases = listOf(false, true).flatMap { dark -> Accents.all.map { Case(dark, it.id) } }

    /**
     * The worst contrast of the pair [pick] chooses over every accent, in both appearances or only the dark ([dark] = true) or light ([dark] = false) one;
     * printed, and held at [floor].
     */
    private fun worst(name: String, floor: Float, dark: Boolean? = null, pick: (Case) -> Pair<Color, Color>) {
        var min = Float.MAX_VALUE
        var at = ""
        for (case in cases) {
            if (dark != null && case.dark != dark) continue
            val (a, b) = pick(case)
            val ratio = contrastRatio(a, b)
            if (ratio < min) { min = ratio; at = case.toString() }
        }
        val label = when (dark) { null -> name; true -> "$name [dark]"; false -> "$name [light]" }
        println(String.format(Locale.ROOT, "contrast %-52s min %5.2f  (%s)  floor %.1f", label, min, at, floor))
        assertTrue("$label is $min at $at, below $floor", min >= floor)
    }

    private val surfaces: List<Pair<String, (ColorScheme) -> Color>> = listOf(
        "surface" to { it.surface }, "surfaceVariant" to { it.surfaceVariant }, "surfaceDim" to { it.surfaceDim }, "surfaceBright" to { it.surfaceBright },
        "surfaceContainerLowest" to { it.surfaceContainerLowest }, "surfaceContainerLow" to { it.surfaceContainerLow }, "surfaceContainer" to { it.surfaceContainer },
        "surfaceContainerHigh" to { it.surfaceContainerHigh }, "surfaceContainerHighest" to { it.surfaceContainerHighest },
    )

    @Test fun inkOnEachToneAndItsContainerReads() {
        worst("onPrimary / primary", 4.5f) { it.scheme.onPrimary to it.scheme.primary }
        worst("onSecondary / secondary", 4.5f) { it.scheme.onSecondary to it.scheme.secondary }
        worst("onTertiary / tertiary", 4.5f) { it.scheme.onTertiary to it.scheme.tertiary }
        worst("onError / error", 4.5f) { it.scheme.onError to it.scheme.error }
        worst("onPrimaryContainer / primaryContainer", 4.5f) { it.scheme.onPrimaryContainer to it.scheme.primaryContainer }
        worst("onSecondaryContainer / secondaryContainer", 4.5f) { it.scheme.onSecondaryContainer to it.scheme.secondaryContainer }
        worst("onTertiaryContainer / tertiaryContainer", 4.5f) { it.scheme.onTertiaryContainer to it.scheme.tertiaryContainer }
        worst("onErrorContainer / errorContainer", 4.5f) { it.scheme.onErrorContainer to it.scheme.errorContainer }
        worst("onBackground / background", 4.5f) { it.scheme.onBackground to it.scheme.background }
        worst("inverseOnSurface / inverseSurface", 4.5f) { it.scheme.inverseOnSurface to it.scheme.inverseSurface }
        worst("inversePrimary / inverseSurface", 4.5f) { it.scheme.inversePrimary to it.scheme.inverseSurface }
    }

    @Test fun bodyAndMutedInkReadOnEverySurfaceRung() {
        for ((name, surface) in surfaces) {
            worst("onSurface / $name", 4.5f) { it.scheme.onSurface to surface(it.scheme) }
            worst("onSurfaceVariant / $name", 4.5f) { it.scheme.onSurfaceVariant to surface(it.scheme) }
        }
        // The stream itself, per appearance: Appendix D lists light and dark apart.
        for (dark in listOf(false, true)) {
            worst("onSurface / surface", 4.5f, dark) { it.scheme.onSurface to it.scheme.surface }
            worst("onSurfaceVariant / surface", 4.5f, dark) { it.scheme.onSurfaceVariant to it.scheme.surface }
        }
    }

    @Test fun theAccentAndErrorAreLegibleAsTextWhereTheyAreUsedAsText() {
        // Links, text buttons, the unread divider and a destructive confirm sit on the stream, a list, a banner or a dialog.
        for (name in listOf("surface", "surfaceContainerLow", "surfaceContainer", "surfaceContainerHigh")) {
            val surface = surfaces.first { it.first == name }.second
            worst("primary (text) / $name", 4.5f) { it.scheme.primary to surface(it.scheme) }
            worst("error (text) / $name", 4.5f) { it.scheme.error to surface(it.scheme) }
        }
    }

    @Test fun theRunCardTonesAreLegibleAsGraphics() {
        // U4: ok (Completed), danger (Failed) and muted (every other state), as 3:1 graphics on the stream and on a card.
        for (name in listOf("surface", "surfaceContainerLow", "surfaceContainerHigh")) {
            val surface = surfaces.first { it.first == name }.second
            worst("ok (graphic) / $name", 3f) { it.palette.ok to surface(it.scheme) }
            worst("danger = error (graphic) / $name", 3f) { it.scheme.error to surface(it.scheme) }
            worst("muted = onSurfaceVariant (graphic) / $name", 3f) { it.scheme.onSurfaceVariant to surface(it.scheme) }
        }
        for (dark in listOf(false, true)) {
            worst("ok (graphic) / surface", 3f, dark) { it.palette.ok to it.scheme.surface }
            worst("danger = error (graphic) / surface", 3f, dark) { it.scheme.error to it.scheme.surface }
            worst("muted = onSurfaceVariant (graphic) / surface", 3f, dark) { it.scheme.onSurfaceVariant to it.scheme.surface }
        }
    }

    @Test fun theOwnMessageTintKeepsTextReadable() {
        // U3: the accent at 6 percent over the stream; body text (onSurface) and the muted time (onSurfaceVariant) must still read at 4.5:1.
        worst("onSurface / own-message tint", 4.5f) { it.scheme.onSurface to tinted(it) }
        worst("onSurfaceVariant / own-message tint", 4.5f) { it.scheme.onSurfaceVariant to tinted(it) }
        for (dark in listOf(false, true)) {
            worst("onSurface / own-message tint", 4.5f, dark) { it.scheme.onSurface to tinted(it) }
            worst("onSurfaceVariant / own-message tint", 4.5f, dark) { it.scheme.onSurfaceVariant to tinted(it) }
        }
    }

    /** The row as drawn: `primary` at [OWN_MESSAGE_TINT] alpha, source-over the stream's surface. */
    private fun tinted(case: Case): Color = case.scheme.primary.copy(alpha = OWN_MESSAGE_TINT).compositeOver(case.scheme.surface)

    @Test fun theSurfaceLadderRunsTheRightWay() {
        for (case in cases) {
            val s = case.scheme
            val rungs = listOf(s.surfaceContainerLowest, s.surfaceContainerLow, s.surfaceContainer, s.surfaceContainerHigh, s.surfaceContainerHighest).map { it.luminance() }
            val sign = if (case.dark) 1f else -1f
            rungs.zipWithNext().forEach { (lower, upper) ->
                assertTrue("the surface ladder must climb away from the window in ${case.dark} $case", sign * (upper - lower) > 0f)
            }
            // Dim is never lighter than the window and bright never darker.
            assertTrue("dim <= surface in $case", s.surfaceDim.luminance() <= s.surface.luminance() + 1e-4f)
            assertTrue("bright >= surface in $case", s.surfaceBright.luminance() >= s.surface.luminance() - 1e-4f)
        }
    }

    @Test fun containersFollowTheChosenAccentAndTheOthersDoNot() {
        for (dark in listOf(false, true)) {
            val schemes = Accents.all.map { resolvePalette(dark, it.id).toColorScheme() }
            // A chip, a tonal button, the selected tab and the jump pill take the accent...
            assertEquals("primaryContainer, dark=$dark", Accents.all.size, schemes.map { it.primaryContainer }.toSet().size)
            assertEquals("secondaryContainer, dark=$dark", Accents.all.size, schemes.map { it.secondaryContainer }.toSet().size)
            // ...while the tertiary and error containers wash their own tones, whatever the accent.
            assertEquals(1, schemes.map { it.tertiaryContainer }.toSet().size)
            assertEquals(1, schemes.map { it.errorContainer }.toSet().size)
        }
    }

    @Test fun noRoleKeepsTheStockPurpleOfAnotherAppearance() {
        // The stock schemes tint everything lavender; a role copied from the other appearance would flash the wrong surface.
        val light = resolvePalette(false, Accents.DEFAULT_ID).toColorScheme()
        val dark = resolvePalette(true, Accents.DEFAULT_ID).toColorScheme()
        assertNotEquals(light.surface, dark.surface)
        assertNotEquals(light.surfaceContainerHighest, dark.surfaceContainerHighest)
        assertNotEquals(light.inverseSurface, dark.inverseSurface)
        assertNotEquals(light.errorContainer, dark.errorContainer)
        assertNotEquals(light.tertiaryContainer, dark.tertiaryContainer)
    }

    @Test fun inkOnAToneIsTheWindowOrTextColourWhicheverReadsBetter() {
        for (case in cases) {
            val p = case.palette
            for (tone in listOf(p.accent, p.accent2, p.accent3, p.danger)) {
                val ink = p.onTone(tone)
                assertTrue(ink == p.windowBg || ink == p.text)
                assertTrue(contrastRatio(ink, tone) >= contrastRatio(if (ink == p.windowBg) p.text else p.windowBg, tone))
            }
        }
    }
}
