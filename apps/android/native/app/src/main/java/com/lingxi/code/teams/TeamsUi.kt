package com.lingxi.code.teams

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.text.format.DateFormat
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CloudOff
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.graphics.vector.ImageVector
import com.lingxi.code.R
import kotlinx.coroutines.delay
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/** Test tags of the Teams tab (this slice owns them; the shell only owns the four tab tags). */
internal object TeamsTags {
    const val ROOT = "teams.root"
    const val SIGN_IN = "teams.signin"
    const val LOADING = "teams.loading"
    const val LOAD_FAILED = "teams.loadFailed"
    const val EMPTY = "teams.empty"
    const val TEAM_CHIP = "teams.teamChip"
    const val SEARCH = "teams.search"
    const val SEARCH_FIELD = "teams.searchField"
    const val NEW = "teams.new"
    const val OFFLINE = "teams.offline"
    const val ERROR = "teams.error"
    const val INVALID_DATA = "teams.invalidData"
    const val LIST = "teams.list"
    const val NO_CONVERSATIONS = "teams.noConversations"
    const val NO_MATCHES = "teams.noMatches"
    const val CONVERSATION = "teams.conversation"
    const val STREAM = "teams.stream"
    const val COMPOSER_FIELD = "teams.composer"
    const val SEND = "teams.send"
    const val BACK = "teams.back"
    const val INFO = "teams.info"
    const val PILL = "teams.pill"
    const val UNREAD_DIVIDER = "teams.unreadDivider"
    const val COMPOSER_LOCK = "teams.composerLock"
    const val ASK_BOT = "teams.askBot"
    const val SHEET = "teams.sheet"
    fun section(id: String) = "teams.section.$id"
    fun row(id: String) = "teams.row.$id"
    fun message(id: String) = "teams.message.$id"
    fun runCard(trigger: String) = "teams.run.$trigger"
}

/** The words of the conversation list, from resources for the UI language (the two separators are code constants). */
@Composable
internal fun rememberTeamsListStrings(): TeamsListStrings {
    val chinese = TeamsListStrings.isChinese(LocalConfiguration.current.locales[0].toLanguageTag())
    return TeamsListStrings(
        channel = stringResource(R.string.teams_conversation_channel), directMessage = stringResource(R.string.teams_label_direct),
        group = stringResource(R.string.teams_conversation_group), member = stringResource(R.string.teams_member),
        you = stringResource(R.string.teams_preview_you), bot = "Bot", noMessages = stringResource(R.string.teams_preview_none),
        recalled = stringResource(R.string.teams_message_recalled), nonText = stringResource(R.string.teams_preview_non_text),
        listSeparator = if (chinese) TeamsListStrings.ZH_LIST_SEPARATOR else ", ",
        authorSeparator = if (chinese) TeamsListStrings.ZH_AUTHOR_SEPARATOR else ": ",
    )
}

/** Error code (HTTP or local) to text (SPEC-mobile-team-bot-ui section 6.7). */
@StringRes
internal fun teamsErrorRes(code: String): Int = when {
    code == "INVALID_DATA" || code == "INVALID_MESSAGE_SNAPSHOT" -> R.string.teams_error_invalid_data
    code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED") -> R.string.teams_stale_error
    code.contains("STORAGE") -> R.string.teams_messages_storage_error
    code in setOf("BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "BOT_SELECTION_STALE") -> R.string.teams_error_bot_access_changed
    code in setOf("QUESTION_RECIPIENT_REQUIRED", "ANSWER_CONFLICT", "INVALID_ANSWER") -> R.string.teams_error_answer_rejected
    code == "MESSAGE_LISTENER_FAILED" -> R.string.teams_error_sync_failed
    code in setOf("MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "INVALID_INPUT") -> R.string.teams_message_failed
    code == "INVITE_UNAVAILABLE" -> R.string.teams_invite_error
    else -> R.string.teams_error
}

@Composable
internal fun teamRoleLabel(role: String): String = stringResource(
    when (role) { "owner" -> R.string.teams_owner; "admin" -> R.string.teams_admin; else -> R.string.teams_member },
)

/** Puts text on the system clipboard as plain text (the Compose clipboard manager this replaces is deprecated). */
@Composable
internal fun rememberCopyToClipboard(label: String): (String) -> Unit {
    val context = LocalContext.current
    return remember(context, label) {
        { text: String -> (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText(label, text)) }
    }
}

/** The wall clock as state, refreshed every [periodMs]; read `.value` only where it is needed so little recomposes. */
@Composable
internal fun rememberNowState(periodMs: Long): State<Long> {
    val now = remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(periodMs) { while (true) { delay(periodMs); now.longValue = System.currentTimeMillis() } }
    return now
}

/** The wall clock, refreshed every [periodMs] so "today" and list times stay right while the screen is open. */
@Composable
internal fun rememberNowMs(periodMs: Long = 30_000L): Long = rememberNowState(periodMs).value

/** The list's time column: clock today, "Yesterday", short weekday, then the date; blank without a time. */
@Composable
internal fun listTimeText(atMs: Long?, nowMs: Long): String {
    val zone = remember { ZoneId.systemDefault() }
    val locale = LocalConfiguration.current.locales[0]
    val yesterday = stringResource(R.string.session_relative_yesterday)
    return when (val kind = TeamsStreamLogic.listTimeKind(atMs, nowMs, zone)) {
        ListTimeKind.None -> ""
        ListTimeKind.Clock -> TeamsStreamLogic.formatClock(atMs, zone, locale)
        ListTimeKind.Yesterday -> yesterday
        ListTimeKind.Weekday -> formatDate(atMs!!, zone, locale, "EEE")
        ListTimeKind.DateThisYear, ListTimeKind.DateOtherYear -> formatDate(atMs!!, zone, locale, if (kind == ListTimeKind.DateThisYear) "MMMd" else "yMMMd")
    }
}

/** The date separator's text: "Today", "Yesterday", or weekday + month + day (+ year). */
@Composable
internal fun dayLabelText(kind: DayKind, atMs: Long): String {
    val zone = remember { ZoneId.systemDefault() }
    val locale = LocalConfiguration.current.locales[0]
    val today = stringResource(R.string.drawer_recents_today)
    val yesterday = stringResource(R.string.session_relative_yesterday)
    return when (kind) {
        DayKind.Today -> today
        DayKind.Yesterday -> yesterday
        DayKind.DateThisYear -> formatDate(atMs, zone, locale, "EEEEMMMMd")
        DayKind.DateOtherYear -> formatDate(atMs, zone, locale, "EEEEMMMMdy")
    }
}

private fun formatDate(atMs: Long, zone: ZoneId, locale: Locale, skeleton: String): String =
    DateTimeFormatter.ofPattern(DateFormat.getBestDateTimePattern(locale, skeleton), locale).withZone(zone).format(Instant.ofEpochMilli(atMs))

/** "Offline · saved messages" with the reason and a Reconnect action. */
@Composable
internal fun TeamsOfflineBanner(onReconnect: () -> Unit, modifier: Modifier = Modifier) {
    Surface(modifier.fillMaxWidth().testTag(TeamsTags.OFFLINE), color = MaterialTheme.colorScheme.surfaceVariant) {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.Outlined.CloudOff, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.teams_offline_title), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurface)
                Text(stringResource(R.string.teams_messages_offline), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            TextButton(onClick = onReconnect) { Text(stringResource(R.string.mcp_reconnect)) }
        }
    }
}

/** A command or sync error, with an optional Retry. */
@Composable
internal fun TeamsErrorBanner(code: String, modifier: Modifier = Modifier, onRetry: (() -> Unit)? = null) {
    Surface(modifier.fillMaxWidth().testTag(TeamsTags.ERROR), color = MaterialTheme.colorScheme.errorContainer) {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.Outlined.ErrorOutline, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onErrorContainer)
            Text(stringResource(teamsErrorRes(code)), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onErrorContainer)
            if (onRetry != null) TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
        }
    }
}

/** A centred message: an optional icon, a title, an optional body and an optional action. */
@Composable
internal fun TeamsMessageState(
    title: String, modifier: Modifier = Modifier, icon: ImageVector? = null, body: String? = null, action: (@Composable () -> Unit)? = null,
) {
    Column(modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        if (icon != null) { Icon(icon, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant); Spacer(Modifier.height(12.dp)) }
        Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        if (body != null) { Spacer(Modifier.height(6.dp)); Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center) }
        if (action != null) { Spacer(Modifier.height(16.dp)); action() }
    }
}
