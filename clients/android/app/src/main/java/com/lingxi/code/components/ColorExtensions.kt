package com.lingxi.code.components

import androidx.compose.ui.graphics.Color

/**
 * color-mix / tint helpers, ported from the iOS `SharedComponents.swift`.
 *
 * The prototype leans on CSS `color-mix(in oklab, A p%, B)`. We approximate it
 * with straight (s)RGB interpolation — visually indistinguishable for the small
 * tints used here.
 */

/** `mix(this, other, amount)` — `amount` fraction of `this` blended onto `other`. */
fun Color.mix(other: Color, amount: Float): Color {
    val p = amount.coerceIn(0f, 1f)
    return Color(
        red = red * p + other.red * (1 - p),
        green = green * p + other.green * (1 - p),
        blue = blue * p + other.blue * (1 - p),
        alpha = alpha * p + other.alpha * (1 - p),
    )
}

/**
 * `color-mix(in oklab, this p%, transparent)` — i.e. this color at `amount`
 * of its current opacity.
 */
fun Color.tint(amount: Float): Color =
    copy(alpha = alpha * amount.coerceIn(0f, 1f))
