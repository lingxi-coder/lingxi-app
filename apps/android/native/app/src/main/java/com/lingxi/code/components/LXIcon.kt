package com.lingxi.code.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowForward
import androidx.compose.material.icons.outlined.Forum
import androidx.compose.material.icons.outlined.Menu
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.AutoAwesome
import androidx.compose.material.icons.automirrored.outlined.MenuBook
import androidx.compose.material.icons.outlined.AccountTree
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material.icons.outlined.GraphicEq
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.LightMode
import androidx.compose.material.icons.outlined.DarkMode
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.Checklist
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.Psychology
import androidx.compose.material.icons.outlined.ArrowUpward
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.ChatBubbleOutline
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.Pause
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Extension
import androidx.compose.material.icons.outlined.Power
import androidx.compose.material.icons.outlined.Bedtime
import androidx.compose.material.icons.outlined.Link
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.Lightbulb
import androidx.compose.material.icons.outlined.Apps
import androidx.compose.material.icons.outlined.Construction
import androidx.compose.material.icons.outlined.Groups
import androidx.compose.material.icons.outlined.Person
import androidx.compose.material.icons.outlined.SmartToy
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

enum class LXIconName {
    Menu, SideChat, Edit, Search, Sparkle, Book, Workflow, Cog, Plus, Mic, AudioWave, Paperclip,
    Chevron, Sun, Moon, Check, Pin, Brain, ArrowUp, Folder, Clock, Message,
    ChevronR, Play, Pause, X, Skill, Plug, Dream, Link, Copy, Share, ListChecks, Send,
    ShieldTerminal, ArrowRight, Terminal, Lightbulb, Apps, AppBuilder, McpFallback, Globe, Computer,
    /** The Bots tab: a robot head, distinct from [Sparkle] (which already means "AI" elsewhere). */
    Bot,
    /** The Teams tab: a group of people ([Link], which the old Teams drawer row used, is not one). */
    Users,
    /** The Friends tab: one person, as [Users] is a group. */
    User,
}

/** Shield and terminal path shared with the reference refusal row. */
private val shieldTerminalIcon = ImageVector.Builder(
    name = "ShieldTerminal", defaultWidth = 24.dp, defaultHeight = 24.dp,
    viewportWidth = 16f, viewportHeight = 17f,
).apply {
    path(stroke = SolidColor(Color.Black), strokeLineWidth = 1.1f,
        strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round) {
        moveTo(8f, 1f); lineTo(14f, 4f); lineTo(14f, 9f)
        curveTo(14f, 12f, 11.5f, 14f, 8f, 16f)
        curveTo(4.5f, 14f, 2f, 12f, 2f, 9f)
        lineTo(2f, 4f); close()
        moveTo(5f, 6f); lineTo(7f, 8f); lineTo(5f, 10f)
        moveTo(9f, 10f); lineTo(11f, 10f)
    }
}.build()

/** Matches the desktop computer-use glyph on a 16-point grid. */
private val computerIcon = ImageVector.Builder(
    name = "Computer", defaultWidth = 24.dp, defaultHeight = 24.dp,
    viewportWidth = 16f, viewportHeight = 16f,
).apply {
    path(stroke = SolidColor(Color.Black), strokeLineWidth = 1.1f,
        strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round) {
        moveTo(7.25f, 10.75f)
        horizontalLineTo(3.25f)
        arcTo(1.5f, 1.5f, 0f, false, true, 1.75f, 9.25f)
        verticalLineTo(3.75f)
        arcTo(1.5f, 1.5f, 0f, false, true, 3.25f, 2.25f)
        horizontalLineTo(12.75f)
        arcTo(1.5f, 1.5f, 0f, false, true, 14.25f, 3.75f)
        verticalLineTo(6.25f)
        moveTo(6.25f, 10.75f)
        verticalLineTo(13.75f)
        moveTo(4.25f, 13.75f)
        horizontalLineTo(8.25f)
        moveTo(9.25f, 7.25f)
        lineTo(14.25f, 10.5f)
        lineTo(11.75f, 11f)
        lineTo(10.75f, 13.5f)
        close()
    }
}.build()

/** Shared semantic names resolve to Android's Material icon family. */
val LXIconName.materialIcon: ImageVector get() = when (this) {
    LXIconName.Computer -> computerIcon
    LXIconName.McpFallback -> Icons.Outlined.Power
    LXIconName.Menu -> Icons.Outlined.Menu
    LXIconName.SideChat -> Icons.Outlined.Forum
    LXIconName.Edit -> Icons.Outlined.Edit
    LXIconName.Search -> Icons.Outlined.Search
    LXIconName.Sparkle -> Icons.Outlined.AutoAwesome
    LXIconName.Book -> Icons.AutoMirrored.Outlined.MenuBook
    LXIconName.Workflow -> Icons.Outlined.AccountTree
    LXIconName.Cog -> Icons.Outlined.Settings
    LXIconName.Plus -> Icons.Outlined.Add
    LXIconName.Mic -> Icons.Outlined.Mic
    LXIconName.AudioWave -> Icons.Outlined.GraphicEq
    LXIconName.Paperclip -> Icons.Outlined.AttachFile
    LXIconName.Chevron -> Icons.Outlined.KeyboardArrowDown
    LXIconName.Sun -> Icons.Outlined.LightMode
    LXIconName.Moon -> Icons.Outlined.DarkMode
    LXIconName.ListChecks -> Icons.Outlined.Checklist
    LXIconName.Send -> Icons.AutoMirrored.Outlined.Send
    LXIconName.Check -> Icons.Outlined.Check
    LXIconName.Pin -> Icons.Outlined.PushPin
    LXIconName.Brain -> Icons.Outlined.Psychology
    LXIconName.ArrowUp -> Icons.Outlined.ArrowUpward
    LXIconName.Folder -> Icons.Outlined.Folder
    LXIconName.Clock -> Icons.Outlined.Schedule
    LXIconName.Message -> Icons.Outlined.ChatBubbleOutline
    LXIconName.ChevronR -> Icons.AutoMirrored.Outlined.KeyboardArrowRight
    LXIconName.Play -> Icons.Outlined.PlayArrow
    LXIconName.Pause -> Icons.Outlined.Pause
    LXIconName.X -> Icons.Outlined.Close
    LXIconName.Skill -> Icons.Outlined.Extension
    LXIconName.Plug -> Icons.Outlined.Power
    LXIconName.Dream -> Icons.Outlined.Bedtime
    LXIconName.Link -> Icons.Outlined.Link
    LXIconName.Globe -> Icons.Outlined.Language
    LXIconName.Copy -> Icons.Outlined.ContentCopy
    LXIconName.Share -> Icons.Outlined.Share
    LXIconName.ShieldTerminal -> shieldTerminalIcon
    LXIconName.Terminal -> Icons.Outlined.Terminal
    LXIconName.Lightbulb -> Icons.Outlined.Lightbulb
    LXIconName.Apps -> Icons.Outlined.Apps
    LXIconName.AppBuilder -> Icons.Outlined.Construction
    LXIconName.ArrowRight -> Icons.AutoMirrored.Outlined.ArrowForward
    LXIconName.Bot -> Icons.Outlined.SmartToy
    LXIconName.Users -> Icons.Outlined.Groups
    LXIconName.User -> Icons.Outlined.Person
}

@Suppress("UNUSED_PARAMETER")
@Composable
fun LXIcon(
    name: LXIconName,
    modifier: Modifier = Modifier,
    size: Dp = 24.dp,
    color: Color = androidx.compose.material3.MaterialTheme.colorScheme.onSurfaceVariant,
    stroke: Float = 1.7f,
    contentDescription: String? = null,
) {
    if (name == LXIconName.McpFallback) {
        Canvas(modifier.size(size)) {
            val scale = this.size.width / 16f
            for (x in listOf(4.5f, 11.5f)) for (y in listOf(4.5f, 11.5f)) {
                drawCircle(color, radius = 2.25f * scale, center = Offset(x * scale, y * scale), style = Stroke(1.05f * scale))
            }
            drawLine(color, Offset(6.1f * scale, 9.9f * scale), Offset(9.9f * scale, 6.1f * scale), strokeWidth = 1.05f * scale)
        }
    } else {
        Icon(name.materialIcon, contentDescription, modifier.size(size), tint = color)
    }
}
