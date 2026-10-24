package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.apps.AppCover
import com.lingxi.code.theme.LingXiTheme
import org.json.JSONObject

/** The app a `LocalApp` publish stored, decoded at the live and persisted result boundaries. */
@Immutable
data class PublishedLocalApp(val id: String, val name: String, val revision: Int)

private val LocalAppId = Regex("a[a-f0-9]{32}")

/**
 * The card a `LocalApp` result draws, or null. Only a publish has one: `list`
 * and `read` stay ordinary tool rows. The result is the tool's small
 * structured output (`{action, id, revision, name, warnings}`), never the app.
 */
internal fun parseLocalAppResult(tool: String, json: String): PublishedLocalApp? {
    if (tool != "LocalApp") return null
    return runCatching {
        val output = JSONObject(json)
        if (output.optString("action", "publish") != "publish") return null
        val id = output.getString("id")
        require(LocalAppId.matches(id))
        val revision = output.getInt("revision")
        require(revision >= 1)
        val name = output.getString("name")
        require(name.isNotBlank())
        PublishedLocalApp(id, name, revision)
    }.getOrNull()
}

/** What a [LocalAppCard] needs from the screen that hosts the conversation. */
class LocalAppCardLinks(
    /** Opens the app full screen. */
    val open: (String) -> Unit,
    /** Whether the app is still in the library; null when that cannot be told. */
    val exists: suspend (String) -> Boolean?,
    /** Changes whenever the library does, so every card asks again. */
    val generation: Any?,
)

val LocalAppCardHost = compositionLocalOf<LocalAppCardLinks?> { null }

/** A published app in the conversation: tap it to open the app. */
@Composable
internal fun LocalAppCard(app: PublishedLocalApp, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    val links = LocalAppCardHost.current
    var exists by remember(app.id) { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(app.id, links?.generation) { exists = links?.exists?.invoke(app.id) }
    // Until the library answers, the app is assumed to be there.
    val openable = links != null && exists != false
    val shape = RoundedCornerShape(14.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(t.surface)
            .border(0.5.dp, t.border, shape)
            .testTag("conversation.localApp.${app.id}")
            .then(if (openable) Modifier.clickable(role = Role.Button) { links.open(app.id) } else Modifier)
            .padding(12.dp),
    ) {
        AppCover(app.name, category = "", size = 44.dp)
        Column(Modifier.weight(1f)) {
            Text(
                text = app.name,
                color = t.text,
                fontSize = androidx.compose.material3.MaterialTheme.typography.bodyLarge.fontSize,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = if (exists == false) {
                    stringResource(R.string.chat_local_app_deleted)
                } else {
                    "${stringResource(R.string.chat_local_app_kind)} · ${stringResource(R.string.apps_version_fmt, app.revision)}"
                },
                color = t.text3,
                fontSize = androidx.compose.material3.MaterialTheme.typography.bodySmall.fontSize,
                maxLines = 1,
            )
        }
        if (openable) {
            Text(
                text = stringResource(R.string.common_open),
                color = t.accent,
                fontSize = androidx.compose.material3.MaterialTheme.typography.bodyMedium.fontSize,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(50))
                    .background(t.accent.copy(alpha = 0.12f))
                    .padding(horizontal = 14.dp, vertical = 7.dp),
            )
        }
    }
}
