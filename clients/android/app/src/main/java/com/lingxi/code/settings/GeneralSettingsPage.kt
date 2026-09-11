package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.lingxi.code.model.ProviderKind

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

@Composable
internal fun NativeNotificationsPage() {
    val context = androidx.compose.ui.platform.LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(androidx.compose.ui.res.stringResource(com.lingxi.code.R.string.settings_notifications_footer))
        TextButton(onClick = {
            context.startActivity(android.content.Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, context.packageName))
        }) { Text(androidx.compose.ui.res.stringResource(com.lingxi.code.R.string.settings_notifications)) }
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
