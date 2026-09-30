package com.lingxi.code.components

import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.lingxi.code.theme.LingXiTheme

/**
 * LXToggle — the brand switch, ported from the iOS `LXToggle` (matches the
 * prototype's 44×26 track with a 22dp knob and a 0.2s ease-in-out slide).
 *
 * On vs off track color mirrors the iOS version: accent when on, a faint text4
 * tint when off. State is hoisted — the caller owns [checked] and reacts in
 * [onCheckedChange].
 */
@Composable
fun LXToggle(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val t = LingXiTheme.palette
    val trackShape = RoundedCornerShape(99.dp)
    val track = if (checked) t.accent else t.text4.tint(0.35f)
    // 44 track − 22 knob − 2*2 padding = 16dp of travel.
    val knobOffset by animateDpAsState(
        targetValue = if (checked) 16.dp else 0.dp,
        animationSpec = tween(durationMillis = 200),
        label = "lxToggleKnob",
    )
    val interaction = remember { MutableInteractionSource() }

    Box(
        modifier = modifier
            .size(width = 44.dp, height = 26.dp)
            .background(track, trackShape)
            .clickable(
                interactionSource = interaction,
                indication = null,
                enabled = enabled,
            ) { onCheckedChange(!checked) }
            .padding(2.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        Surface(
            color = Color.White,
            shape = CircleShape,
            shadowElevation = 1.5.dp,
            modifier = Modifier
                .offset(x = knobOffset)
                .size(22.dp),
        ) {}
    }
}
