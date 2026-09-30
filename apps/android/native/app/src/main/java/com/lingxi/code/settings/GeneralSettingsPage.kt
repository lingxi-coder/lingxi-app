package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.notify.NotificationPolicy

@Composable
internal fun GeneralSettingsPage(onNavigate: (String) -> Unit, onReplayOnboarding: () -> Unit) {
    val pages = listOf(
        "Language" to SettingsRoutes.LANGUAGE,
        "Notifications" to SettingsRoutes.NOTIFICATIONS,
        "Keyboard & input" to SettingsRoutes.INPUT,
        "Data & privacy" to SettingsRoutes.PRIVACY,
        "Voice & audio" to SettingsRoutes.VOICE,
        "Web search providers" to SettingsRoutes.providerList(ProviderKind.Search.name),
        "Web fetch providers" to SettingsRoutes.providerList(ProviderKind.Fetch.name),
        "Permission mode" to SettingsRoutes.PERMISSION_MODE,
        "TypeScript language server" to SettingsRoutes.TYPESCRIPT_LSP,
        "Linux runtime" to SettingsRoutes.LINUX_RUNTIME,
        "Computer use" to SettingsRoutes.COMPUTER_USE,
        "Scheduled tasks" to SettingsRoutes.CRON,
        "Bundled skills & local apps" to SettingsRoutes.SKILLS,
    )
    SettingsSection(label = settingsLabel("Device & mobile capabilities")) {
        pages.filter { it.second != SettingsRoutes.COMPUTER_USE || com.lingxi.code.computeruse.ComputerUseFeatureProvider.available }.forEach { (title, route) -> TextButton(onClick = { onNavigate(route) }) { Text(settingsLabel(title)) } }
        TextButton(onClick = onReplayOnboarding) { Text(settingsLabel("Replay onboarding")) }
    }
}

@Composable
internal fun NativeAccountPage(state: SettingsUiState, onCredentials: () -> Unit) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(settingsLabel("Provider accounts"))
        Text(settingsLabel("This installation connects using your configured provider credentials. Manage credentials to connect or change an account."))
        state.llmProviders.forEach { provider -> Text(settingsLabel("${provider.name} · ${provider.status}")) }
        TextButton(onClick = onCredentials) { Text(settingsLabel("Manage provider credentials")) }
    }
}

/**
 * Real notification preferences, not a deep link out to the system screen.
 *
 * This used to be one button that opened Android's own per-app notification
 * settings, which can only turn the app's notifications off wholesale. The
 * toggles here choose WHICH of the four moments are worth interrupting for —
 * a distinction the OS screen has no way to express.
 *
 * The idle threshold and the 6-second permission delay are upstream Claude
 * Code's, not ours; see [com.lingxi.code.notify.NotificationPolicy].
 */
@Composable
internal fun NativeNotificationsPage(
    config: NotifConfig,
    onChange: (NotifConfig) -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SettingsSection(
            label = stringResource(R.string.settings_notifications),
            footer = stringResource(R.string.settings_notifications_footer),
        ) {
            NotifToggleRow(
                title = stringResource(R.string.settings_notif_enabled),
                subtitle = stringResource(R.string.settings_notif_enabled_sub),
                checked = config.enabled,
                onCheckedChange = { onChange(config.copy(enabled = it)) },
            )
        }
        SettingsSection(label = stringResource(R.string.settings_section_notification_type)) {
            NotifToggleRow(
                title = stringResource(R.string.settings_notif_idle_prompt),
                subtitle = stringResource(R.string.settings_notif_idle_prompt_sub),
                checked = config.idlePromptNotifEnabled,
                enabled = config.enabled,
                onCheckedChange = { onChange(config.copy(idlePromptNotifEnabled = it)) },
            )
            NotifToggleRow(
                title = stringResource(R.string.settings_notif_needs_input),
                subtitle = stringResource(R.string.settings_notif_needs_input_sub),
                checked = config.inputNeededNotifEnabled,
                enabled = config.enabled,
                onCheckedChange = { onChange(config.copy(inputNeededNotifEnabled = it)) },
            )
            NotifToggleRow(
                title = stringResource(R.string.settings_notif_background_task),
                subtitle = stringResource(R.string.settings_notif_background_task_sub),
                checked = config.taskCompleteNotifEnabled,
                enabled = config.enabled,
                onCheckedChange = { onChange(config.copy(taskCompleteNotifEnabled = it)) },
            )
            NotifToggleRow(
                title = stringResource(R.string.settings_notif_cron_report),
                subtitle = stringResource(R.string.settings_notif_cron_report_sub),
                checked = config.scheduledRunNotifEnabled,
                enabled = config.enabled,
                onCheckedChange = { onChange(config.copy(scheduledRunNotifEnabled = it)) },
            )
        }
        SettingsSection(
            label = stringResource(R.string.settings_notif_idle_threshold),
            footer = stringResource(R.string.settings_notif_idle_threshold_sub),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                idleThresholdChoices().forEach { (ms, label) ->
                    FilterChip(
                        selected = config.messageIdleNotifThresholdMs == ms,
                        enabled = config.enabled,
                        onClick = { onChange(config.copy(messageIdleNotifThresholdMs = ms)) },
                        label = { Text(label) },
                    )
                }
            }
        }
        // Kept: the OS master switch still overrides everything above, and
        // POST_NOTIFICATIONS can only be re-granted from there.
        TextButton(onClick = {
            context.startActivity(
                android.content.Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, context.packageName),
            )
        }) { Text(stringResource(R.string.settings_notif_open_system)) }
    }
}

/**
 * The idle-threshold choices, in render order. Pure and internal so the set and
 * its ordering can be pinned by a JVM test without composing anything.
 *
 * 1 minute is upstream's `messageIdleNotifThresholdMs` default; the others
 * exist because a threshold that can only be the default is not a setting.
 */
internal fun idleThresholdChoices(): List<Pair<Long, String>> = listOf(
    15_000L to "15s",
    NotificationPolicy.DEFAULT_IDLE_NOTIF_THRESHOLD_MS to "1m",
    180_000L to "3m",
    600_000L to "10m",
)

@Composable
private fun NotifToggleRow(
    title: String,
    subtitle: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    enabled: Boolean = true,
) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title)
            Text(subtitle, style = MaterialTheme.typography.bodySmall)
        }
        // Show the value that is actually stored. Masking it with `&& enabled`
        // made every sub-toggle read as off while the master switch was off,
        // so the screen could not tell a genuinely-off type from one that is
        // merely suppressed — and they all snapped back on when the master was
        // re-enabled. `enabled` already blocks interaction.
        Switch(checked = checked, onCheckedChange = onCheckedChange, enabled = enabled)
    }
}

@Composable
internal fun NativeSystemSettingsPage(input: Boolean) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val title = androidx.compose.ui.res.stringResource(if (input) com.lingxi.code.R.string.settings_keyboard_input else com.lingxi.code.R.string.settings_data_privacy)
    TextButton(onClick = {
        val intent = if (input) android.content.Intent(android.provider.Settings.ACTION_INPUT_METHOD_SETTINGS)
        else android.content.Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,android.net.Uri.parse("package:${context.packageName}"))
        context.startActivity(intent)
    }) { Text(title) }
}
