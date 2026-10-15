package com.lingxi.code.components

import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowForward
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
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.Lightbulb
import androidx.compose.material.icons.outlined.Apps
import androidx.compose.material.icons.outlined.Construction
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

enum class LXIconName {
    Menu, Edit, Search, Sparkle, Book, Workflow, Cog, Plus, Mic, AudioWave, Paperclip,
    Chevron, Sun, Moon, Check, Pin, Brain, ArrowUp, Folder, Clock, Message,
    ChevronR, Play, Pause, X, Skill, Plug, Dream, Link, Copy, Share,
    ArrowRight, Terminal, Lightbulb, Apps, AppBuilder,
}

/** Shared semantic names resolve to Android's Material icon family. */
val LXIconName.materialIcon: ImageVector get() = when (this) {
    LXIconName.Menu -> Icons.Outlined.Menu
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
    LXIconName.Copy -> Icons.Outlined.ContentCopy
    LXIconName.Share -> Icons.Outlined.Share
    LXIconName.Terminal -> Icons.Outlined.Terminal
    LXIconName.Lightbulb -> Icons.Outlined.Lightbulb
    LXIconName.Apps -> Icons.Outlined.Apps
    LXIconName.AppBuilder -> Icons.Outlined.Construction
    LXIconName.ArrowRight -> Icons.AutoMirrored.Outlined.ArrowForward
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
    Icon(name.materialIcon, contentDescription, modifier.size(size), tint = color)
}
