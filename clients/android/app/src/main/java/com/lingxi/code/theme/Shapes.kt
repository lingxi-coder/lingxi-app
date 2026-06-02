package com.lingxi.code.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Shapes
import androidx.compose.ui.unit.dp

/**
 * Corner radii used across the brand surfaces, ported from the prototype / iOS.
 *
 * The prototype is radius-specific per component (pills 6, fields 10, cards 12,
 * icon tiles 7, message bubbles 16). Material 3's [Shapes] buckets cover the
 * common cases; surfaces that need an exact value use [LXRadius] directly.
 */
object LXRadius {
    val pill = RoundedCornerShape(6.dp)        // chips / pills
    val iconTile = RoundedCornerShape(7.dp)    // settings-row leading icon tiles
    val field = RoundedCornerShape(10.dp)      // text fields
    val card = RoundedCornerShape(12.dp)       // grouped setting cards
    val bubble = RoundedCornerShape(16.dp)     // chat message bubbles
    val full = RoundedCornerShape(percent = 50) // switches / fully-rounded
}

/** Material 3 shape scale mapped to the brand radii. */
val LXShapes: Shapes = Shapes(
    extraSmall = RoundedCornerShape(6.dp),
    small = RoundedCornerShape(8.dp),
    medium = RoundedCornerShape(12.dp),
    large = RoundedCornerShape(16.dp),
    extraLarge = RoundedCornerShape(22.dp),
)
