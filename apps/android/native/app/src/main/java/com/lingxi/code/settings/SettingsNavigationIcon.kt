package com.lingxi.code.settings

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme

/** Decorative symbols retain the desktop page's meaning in the native icon system. */
@Composable
internal fun SettingsNavigationIcon(route: String) {
    val color = LingXiTheme.palette.text3
    val icon = when (route) {
        SettingsRoutes.GENERAL -> LXIconName.Cog
        SettingsRoutes.APPEARANCE -> LXIconName.Sun
        SettingsRoutes.VOICE -> LXIconName.Mic
        SettingsRoutes.PROJECTS -> LXIconName.Folder
        SettingsRoutes.CUSTOM_PROVIDERS -> LXIconName.Plug
        SettingsRoutes.FUSION, SettingsRoutes.ENGINE_SKILLS -> LXIconName.Sparkle
        SettingsRoutes.HOOKS -> LXIconName.Workflow
        SettingsRoutes.PLUGINS -> LXIconName.Skill
        SettingsRoutes.DIAGNOSTICS -> LXIconName.AudioWave
        else -> null
    }
    if (icon != null) { LXIcon(icon, color = color); return }
    Canvas(Modifier.size(20.dp)) {
        scale(size.width / 24f, size.height / 24f, pivot = Offset.Zero) {
            val pen = Stroke(1.7f)
            when (route) {
                SettingsRoutes.ACCOUNT, SettingsRoutes.CREDENTIALS -> {
                    drawCircle(color,4f,Offset(7f,8f),style=pen)
                    drawLine(color,Offset(10f,11f),Offset(20f,21f),1.7f)
                    drawLine(color,Offset(16f,17f),Offset(19f,14f),1.7f)
                }
                SettingsRoutes.ENGINE_PERMISSIONS -> {
                    val shield = Path().apply { moveTo(12f,2f);lineTo(21f,6f);lineTo(20f,14f);quadraticBezierTo(18f,20f,12f,23f);quadraticBezierTo(6f,20f,4f,14f);lineTo(3f,6f);close() }
                    drawPath(shield,color,style=pen)
                }
                SettingsRoutes.ARCHIVED -> {
                    drawRect(color,Offset(3f,4f),Size(18f,5f),style=pen)
                    drawRect(color,Offset(5f,9f),Size(14f,12f),style=pen)
                    drawLine(color,Offset(9f,13f),Offset(15f,13f),1.7f)
                }
                SettingsRoutes.ENGINE_MCP -> {
                    drawRoundRect(color,Offset(3f,3f),Size(18f,7f),style=pen)
                    drawRoundRect(color,Offset(3f,14f),Size(18f,7f),style=pen)
                    drawCircle(color,1f,Offset(7f,6.5f));drawCircle(color,1f,Offset(7f,17.5f))
                }
                SettingsRoutes.TOOLS_AGENT -> {
                    for (i in 0..2) { val x=5f+i*7f;drawLine(color,Offset(x,3f),Offset(x,21f),1.7f);drawCircle(color,2.5f,Offset(x,if(i==1) 8f else 16f),style=pen) }
                }
                else -> { drawCircle(color,9f,Offset(12f,12f),style=pen);drawLine(color,Offset(12f,10f),Offset(12f,17f),1.7f);drawCircle(color,1f,Offset(12f,6f)) }
            }
        }
    }
}
