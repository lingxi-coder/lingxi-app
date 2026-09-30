package com.lingxi.code.components

import androidx.compose.ui.graphics.Color
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.sin

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

/**
 * Runtime `oklch(L C H / a)` → sRGB [Color]. The pre-computed tokens are baked
 * to sRGB at authoring time, but the FlowMode orb (OrbCanvas) computes colors on
 * the fly (hue shifts, per-particle hues, gradient stops), so it needs a live
 * OKLCH→sRGB conversion. Mirrors the iOS `Color(okl:_:_:_:)` initializer.
 *
 * @param l lightness 0…1 (pass 0.85f for the prototype's `85%`).
 * @param c chroma. @param h hue in degrees. @param alpha opacity 0…1.
 */
fun oklch(l: Float, c: Float, h: Float, alpha: Float = 1f): Color {
    val hr = h * (Math.PI.toFloat() / 180f)
    val a = c * cos(hr)
    val b = c * sin(hr)
    val lp = l + 0.3963377774f * a + 0.2158037573f * b
    val mp = l - 0.1055613458f * a - 0.0638541728f * b
    val sp = l - 0.0894841775f * a - 1.2914855480f * b
    val lc = lp * lp * lp; val mc = mp * mp * mp; val sc = sp * sp * sp
    val rl =  4.0767416621f * lc - 3.3077115913f * mc + 0.2309699292f * sc
    val gl = -1.2684380046f * lc + 2.6097574011f * mc - 0.3413193965f * sc
    val bl = -0.0041960863f * lc - 0.7034186147f * mc + 1.7076147010f * sc
    fun enc(x: Float): Float {
        val v = x.coerceIn(0f, 1f)
        return if (v <= 0.0031308f) 12.92f * v else 1.055f * v.pow(1f / 2.4f) - 0.055f
    }
    return Color(red = enc(rl), green = enc(gl), blue = enc(bl), alpha = alpha.coerceIn(0f, 1f))
}
