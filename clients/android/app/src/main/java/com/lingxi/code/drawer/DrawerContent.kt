package com.lingxi.code.drawer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.model.MockData
import com.lingxi.code.model.Workspace
import com.lingxi.code.theme.LingXiTheme

/**
 * The 对话 / 项目 / 定时 drawer content — the Android analog of the iOS `Drawer`,
 * rendered as the `drawerContent` of a Material 3 `ModalNavigationDrawer` (the
 * gesture, scrim and slide are owned by the M3 component, so this is just the
 * 320dp panel body).
 *
 * State is hoisted into [DrawerUiState]; selecting a chat/session calls
 * [onSelectSession] (which the root shell uses to close the drawer and switch
 * the conversation), and the account row calls [onOpenSettings].
 *
 * Layout mirrors the prototype top-to-bottom: status-bar offset → header →
 * workspace pills → search → section tabs → scrolling section body →
 * knowledge/memory shortcuts → account row.
 */
@Composable
fun DrawerContent(
    ui: DrawerUiState,
    onSelectSession: (String) -> Unit,
    onOpenSettings: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette

    val chats = remember(ui.activeWs) { MockData.chats.filter { it.wsId == ui.activeWs } }
    val projects = remember(ui.activeWs) { MockData.projects.filter { it.wsId == ui.activeWs } }
    val crons = remember(ui.activeWs) { MockData.crons.filter { it.wsId == ui.activeWs } }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.sidebarBg)
            .windowInsetsPadding(WindowInsets.statusBars),
    ) {
        DrawerHeader(onClose = onClose)
        WorkspacePills(activeWs = ui.activeWs, onSelect = ui::selectWorkspace)
        SearchBar()
        SectionTabs(
            section = ui.section,
            chats = chats.size,
            projects = projects.size,
            crons = crons.size,
            onSelect = { ui.section = it },
        )

        // Scrolling section body — fills the space above shortcuts/account.
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp)
                .padding(top = 4.dp, bottom = 8.dp),
        ) {
            when (ui.section) {
                DrawerSection.Chats -> ChatsSection(
                    chats = chats,
                    activeSession = ui.activeSession,
                    onSelectSession = { onSelectSession(it) },
                )

                DrawerSection.Projects -> ProjectsSection(
                    projects = projects,
                    activeSession = ui.activeSession,
                    openProjects = ui.openProjects,
                    onToggleProject = ui::toggleProject,
                    onSelectSession = { onSelectSession(it) },
                )

                DrawerSection.Crons -> CronsSection(crons = crons)
            }
        }

        Shortcuts()
        AccountRow(onClick = onOpenSettings)
    }
}

// MARK: - header ------------------------------------------------------------

@Composable
private fun DrawerHeader(onClose: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 18.dp)
            .padding(top = 8.dp, bottom = 12.dp),
    ) {
        Text("灵犀", color = t.text, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        Box(
            modifier = Modifier
                .size(36.dp)
                .clip(CircleShape)
                .clickable(onClick = onClose),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(name = LXIconName.X, size = 20.dp, color = t.text3, stroke = 1.8f)
        }
    }
}

// MARK: - workspace pills ---------------------------------------------------

@Composable
private fun WorkspacePills(activeWs: String, onSelect: (String) -> Unit) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 18.dp)
            .padding(bottom = 12.dp),
    ) {
        MockData.workspaces.forEach { ws ->
            WorkspacePill(ws = ws, active = ws.id == activeWs, onClick = { onSelect(ws.id) })
        }
    }
}

@Composable
private fun WorkspacePill(ws: Workspace, active: Boolean, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(50)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
        modifier = Modifier
            .clip(shape)
            .background(if (active) ws.color.tint(0.20f) else t.surface)
            .border(0.5.dp, if (active) ws.color.tint(0.35f) else t.border, shape)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 7.dp),
    ) {
        Text(ws.icon, color = if (active) ws.color else t.text3, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Text(ws.name, color = if (active) ws.color else t.text3, fontSize = 13.sp, fontWeight = FontWeight.Medium)
    }
}

// MARK: - search ------------------------------------------------------------

@Composable
private fun SearchBar() {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 18.dp)
            .padding(bottom = 14.dp)
            .clip(shape)
            .background(t.surface)
            .border(0.5.dp, t.border, shape)
            .clickable {}
            .padding(horizontal = 14.dp, vertical = 11.dp),
    ) {
        LXIcon(name = LXIconName.Search, size = 16.dp, color = t.text4, stroke = 2f)
        Text("搜索会话", color = t.text4, fontSize = 14.sp)
    }
}

// MARK: - section tabs ------------------------------------------------------

@Composable
private fun SectionTabs(
    section: DrawerSection,
    chats: Int,
    projects: Int,
    crons: Int,
    onSelect: (DrawerSection) -> Unit,
) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(bottom = 8.dp),
    ) {
        SectionTab(DrawerSection.Chats, LXIconName.Message, "对话", chats, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Projects, LXIconName.Folder, "项目", projects, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Crons, LXIconName.Clock, "定时", crons, section, onSelect, Modifier.weight(1f))
    }
}

@Composable
private fun SectionTab(
    id: DrawerSection,
    icon: LXIconName,
    label: String,
    count: Int,
    current: DrawerSection,
    onSelect: (DrawerSection) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val active = id == current
    val shape = RoundedCornerShape(9.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp, Alignment.CenterHorizontally),
        modifier = modifier
            .clip(shape)
            .background(if (active) t.surfaceActive else Color.Transparent)
            .border(0.5.dp, if (active) t.border else Color.Transparent, shape)
            .clickable { onSelect(id) }
            .testTag(UiTags.drawerTab(id.key))
            .padding(horizontal = 4.dp, vertical = 9.dp),
    ) {
        LXIcon(name = icon, size = 14.dp, color = if (active) t.text else t.text3, stroke = 1.8f)
        Text(
            label,
            color = if (active) t.text else t.text3,
            fontSize = 13.sp,
            fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
        )
        Text(
            count.toString(),
            color = if (active) t.accent else t.text4,
            fontSize = 10.5f.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

// MARK: - shortcuts + account ----------------------------------------------

@Composable
private fun Shortcuts() {
    val t = LingXiTheme.palette
    Row(
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier
            .fillMaxWidth()
            .topBorder(t.border)
            .padding(horizontal = 12.dp)
            .padding(top = 4.dp),
    ) {
        Shortcut(LXIconName.Book, "知识库", 24, Modifier.weight(1f))
        Shortcut(LXIconName.Brain, "记忆", 42, Modifier.weight(1f))
    }
}

@Composable
private fun Shortcut(icon: LXIconName, label: String, count: Int, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = modifier
            .clip(RoundedCornerShape(10.dp))
            .clickable {}
            .padding(horizontal = 14.dp, vertical = 11.dp),
    ) {
        LXIcon(name = icon, size = 15.dp, color = t.text3, stroke = 1.7f)
        Text(label, color = t.text3, fontSize = 13.5f.sp, fontWeight = FontWeight.Medium)
        Spacer(Modifier.weight(1f))
        Text(count.toString(), color = t.text4, fontSize = 11.5f.sp)
    }
}

@Composable
private fun AccountRow(onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .topBorder(t.border)
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .clickable(onClick = onClick)
                .padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .background(Brush.linearGradient(listOf(t.accent, t.accent2))),
                contentAlignment = Alignment.Center,
            ) {
                Text("Y", color = Color.White, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
            Column(modifier = Modifier.weight(1f)) {
                Text("Yuxin Yang", color = t.text, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                Text("Pro · 5.5 / 8 段", color = t.text4, fontSize = 11.5f.sp)
            }
            LXIcon(name = LXIconName.Cog, size = 18.dp, color = t.text3, stroke = 1.6f)
        }
    }
}

/** A 0.5dp hairline drawn along the top edge (the iOS top-`overlay` divider). */
private fun Modifier.topBorder(color: Color): Modifier =
    drawBehind {
        drawLine(
            color = color,
            start = Offset(0f, 0f),
            end = Offset(size.width, 0f),
            strokeWidth = 0.5.dp.toPx(),
        )
    }
