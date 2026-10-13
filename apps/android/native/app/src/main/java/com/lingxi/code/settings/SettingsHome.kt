package com.lingxi.code.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R
import com.lingxi.code.account.AccountViewModel
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.theme.LingXiTheme

/**
 * The phone settings home: a search field, the account card and six groups.
 * Typing in the search field swaps the groups for a flat list of matching pages.
 */
@Composable
internal fun SettingsHomePage(onNavigate: (String) -> Unit) {
    var query by rememberSaveable { mutableStateOf("") }
    val entries = settingsGroupEntries(computerUseAvailable = ComputerUseFeatureProvider.available)
    Column(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.fillMaxWidth()) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text(settingsLabel("Search settings")) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().testTag(UiTags.SETTINGS_SEARCH),
        )
        if (query.isBlank()) {
            SettingsAccountCard(onOpen = { onNavigate(SettingsRoutes.ACCOUNT) })
            SettingsSection {
                SettingsGroup.entries.forEachIndexed { index, group ->
                    SettingsRow(
                        label = stringResource(group.title),
                        sub = stringResource(group.summary),
                        icon = group.icon,
                        isLast = index == SettingsGroup.entries.lastIndex,
                        modifier = Modifier.testTag(UiTags.settingsGroup(group.key)),
                        onTap = { onNavigate(SettingsRoutes.group(group.key)) },
                    )
                }
            }
        } else {
            SettingsSearchResults(entries = entries, query = query, onNavigate = onNavigate)
        }
    }
}

@Composable
private fun SettingsSearchResults(
    entries: List<SettingsGroupEntry>,
    query: String,
    onNavigate: (String) -> Unit,
) {
    val labels = entries.associateWith { settingsLabel(it.title) }
    val groupLabels = SettingsGroup.entries.associateWith { stringResource(it.title) }
    val hits = searchSettingsEntries(
        entries = entries,
        query = query,
        labelOf = { labels.getValue(it) },
        groupLabelOf = { groupLabels.getValue(it) },
    )
    if (hits.isEmpty()) {
        Text(
            text = stringResource(R.string.settings_home_no_results),
            color = LingXiTheme.palette.text4,
            fontSize = 13.sp,
            modifier = Modifier.padding(horizontal = 4.dp, vertical = 8.dp),
        )
        return
    }
    SettingsSection {
        hits.forEachIndexed { index, entry ->
            SettingsRow(
                label = labels.getValue(entry),
                sub = groupLabels.getValue(entry.group),
                isLast = index == hits.lastIndex,
                modifier = Modifier.testTag(UiTags.settingsPage(entry.route)),
                onTap = { onNavigate(entry.route) },
            )
        }
    }
}

/** One group's pages. The About group also carries the replay-onboarding action. */
@Composable
internal fun SettingsGroupPage(
    group: SettingsGroup,
    onNavigate: (String) -> Unit,
    onReplayOnboarding: () -> Unit,
) {
    val entries = settingsGroupEntries(computerUseAvailable = ComputerUseFeatureProvider.available)
        .filter { it.group == group }
    val withReplay = group == SettingsGroup.About
    SettingsSection {
        entries.forEachIndexed { index, entry ->
            SettingsRow(
                label = settingsLabel(entry.title),
                isLast = index == entries.lastIndex && !withReplay,
                modifier = Modifier.testTag(UiTags.settingsPage(entry.route)),
                onTap = { onNavigate(entry.route) },
            )
        }
        if (withReplay) {
            SettingsRow(
                label = stringResource(R.string.settings_rewatch_onboarding),
                sub = stringResource(R.string.settings_rewatch_onboarding_sub),
                isLast = true,
                onTap = onReplayOnboarding,
            )
        }
    }
}

/** The signed-in profile (or a sign-in prompt) — tapping opens the account page. */
@Composable
private fun SettingsAccountCard(onOpen: () -> Unit, model: AccountViewModel = viewModel()) {
    val t = LingXiTheme.palette
    val state by model.state.collectAsStateWithLifecycle()
    val user = state.user
    val title = user?.let { it.name ?: it.email } ?: stringResource(R.string.drawer_account_signed_out_title)
    val subtitle = if (user == null) {
        stringResource(R.string.drawer_account_signed_out_subtitle)
    } else {
        user.email.takeIf { user.name != null }
    }
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
            .testTag(UiTags.SETTINGS_ACCOUNT_CARD)
            .clickable(onClick = onOpen)
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(CircleShape)
                .background(Brush.linearGradient(listOf(t.accent, t.accent2))),
            contentAlignment = Alignment.Center,
        ) {
            val initial = user?.let { (it.name ?: it.email)?.trim()?.firstOrNull()?.uppercase() }
            if (initial != null) {
                Text(initial, color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            } else {
                LXIcon(name = LXIconName.Sparkle, size = 18.dp, color = Color.White, stroke = 1.8f)
            }
        }
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                text = title,
                color = t.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (subtitle != null) {
                Text(
                    text = subtitle,
                    color = t.text4,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        LXIcon(name = LXIconName.ChevronR, size = 13.dp, color = t.text4, stroke = 1.7f)
    }
}
