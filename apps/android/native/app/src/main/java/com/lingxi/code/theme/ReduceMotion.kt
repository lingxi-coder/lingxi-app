package com.lingxi.code.theme

import android.content.ContentResolver
import android.provider.Settings
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext

/** Reduce motion (docs/TEAMS-UI-POLISH.md U10): the system's animator duration scale is 0, which "Remove animations" in Accessibility sets. */
internal fun reduceMotionOf(durationScale: Float): Boolean = durationScale == 0f

/** The device's current setting, read from [resolver]; an absent setting is the normal scale of 1. */
internal fun readReduceMotion(resolver: ContentResolver): Boolean =
    reduceMotionOf(Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f))

/**
 * Whether to skip animation, read once when the caller enters the composition. When it is true, scroll with `scrollToItem` instead of
 * `animateScrollToItem`, give an `AnimatedVisibility` no transition, and show a spinner as its static glyph.
 */
@Composable
internal fun rememberReduceMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    return remember(resolver) { readReduceMotion(resolver) }
}
