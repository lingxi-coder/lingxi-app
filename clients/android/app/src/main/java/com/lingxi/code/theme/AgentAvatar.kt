package com.lingxi.code.theme

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R

/** UTF-16 hash and palette order match the Desktop AgentAvatar. */
fun agentAvatarIndex(agentId: String): Int {
    var hash = 0L
    for (unit in agentId) hash = (hash * 31 + unit.code) % 2147483647L
    return (hash % 28).toInt()
}

private val avatarResources = listOf(
    R.drawable.agent_avatar_00_light to R.drawable.agent_avatar_00_dark,
    R.drawable.agent_avatar_01_light to R.drawable.agent_avatar_01_dark,
    R.drawable.agent_avatar_02_light to R.drawable.agent_avatar_02_dark,
    R.drawable.agent_avatar_03_light to R.drawable.agent_avatar_03_dark,
    R.drawable.agent_avatar_04_light to R.drawable.agent_avatar_04_dark,
    R.drawable.agent_avatar_05_light to R.drawable.agent_avatar_05_dark,
    R.drawable.agent_avatar_06_light to R.drawable.agent_avatar_06_dark,
    R.drawable.agent_avatar_07_light to R.drawable.agent_avatar_07_dark,
    R.drawable.agent_avatar_08_light to R.drawable.agent_avatar_08_dark,
    R.drawable.agent_avatar_09_light to R.drawable.agent_avatar_09_dark,
    R.drawable.agent_avatar_10_light to R.drawable.agent_avatar_10_dark,
    R.drawable.agent_avatar_11_light to R.drawable.agent_avatar_11_dark,
    R.drawable.agent_avatar_12_light to R.drawable.agent_avatar_12_dark,
    R.drawable.agent_avatar_13_light to R.drawable.agent_avatar_13_dark,
    R.drawable.agent_avatar_14_light to R.drawable.agent_avatar_14_dark,
    R.drawable.agent_avatar_15_light to R.drawable.agent_avatar_15_dark,
    R.drawable.agent_avatar_16_light to R.drawable.agent_avatar_16_dark,
    R.drawable.agent_avatar_17_light to R.drawable.agent_avatar_17_dark,
    R.drawable.agent_avatar_18_light to R.drawable.agent_avatar_18_dark,
    R.drawable.agent_avatar_19_light to R.drawable.agent_avatar_19_dark,
    R.drawable.agent_avatar_20_light to R.drawable.agent_avatar_20_dark,
    R.drawable.agent_avatar_21_light to R.drawable.agent_avatar_21_dark,
    R.drawable.agent_avatar_22_light to R.drawable.agent_avatar_22_dark,
    R.drawable.agent_avatar_23_light to R.drawable.agent_avatar_23_dark,
    R.drawable.agent_avatar_24_light to R.drawable.agent_avatar_24_dark,
    R.drawable.agent_avatar_25_light to R.drawable.agent_avatar_25_dark,
    R.drawable.agent_avatar_26_light to R.drawable.agent_avatar_26_dark,
    R.drawable.agent_avatar_27_light to R.drawable.agent_avatar_27_dark
)

@Composable
fun AgentAvatar(agentId: String, modifier: Modifier = Modifier.size(20.dp)) {
    val pair = avatarResources[agentAvatarIndex(agentId)]
    Image(
        painter = painterResource(if (LocalPalette.current.isDark) pair.second else pair.first),
        contentDescription = null,
        modifier = modifier,
    )
}
