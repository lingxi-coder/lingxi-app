package com.lingxi.code.settings

import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.LifecycleResumeEffect
import com.lingxi.code.R
import com.lingxi.code.components.LXToggle
import com.lingxi.code.computeruse.ComputerUseApp
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.computeruse.ComputerUseSessionState
import com.lingxi.code.computeruse.ComputerUseTier
import com.lingxi.code.computeruse.browserAppSelection
import com.lingxi.code.computeruse.browserControlTier
import com.lingxi.code.computeruse.resolveBrowserPackages
import com.lingxi.code.theme.LingXiTheme

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun BrowserSettingsPage(onOpenComputerUse: () -> Unit) {
    val context = LocalContext.current
    val feature = ComputerUseFeatureProvider
    val state by feature.state.collectAsState()
    val configuration by feature.configuration.collectAsState()
    var browsers by remember(context) { mutableStateOf(emptyList<ComputerUseApp>()) }
    var openingSettingsFailed by remember { mutableStateOf(false) }
    val t = LingXiTheme.palette
    val sessionRunning = state.sessionState != ComputerUseSessionState.Inactive
    val browserPackages = remember(browsers) { browsers.mapTo(linkedSetOf()) { it.packageName } }

    LifecycleResumeEffect(context) {
        if (feature.available) {
            val apps = feature.listLaunchableApps(context).filterNot { it.systemUi }
            val packages = resolveBrowserPackages(context, apps.mapTo(linkedSetOf()) { it.packageName })
            browsers = apps.filter { it.packageName in packages }
        }
        onPauseOrDispose { }
    }

    fun changePermission(packageName: String, tier: ComputerUseTier?) {
        val previous = feature.configuration.value
        val next = browserAppSelection(
            configuration = previous,
            sessionState = feature.state.value.sessionState,
            browserPackages = browserPackages,
            packageName = packageName,
            tier = tier,
        )
        if (next != previous) feature.updateConfiguration(context, next)
    }

    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        SettingsSection(
            label = stringResource(R.string.settings_browser_opening),
            footer = stringResource(R.string.settings_browser_default_description),
        ) {
            SettingsRow(
                label = stringResource(R.string.settings_browser_system_default),
                sub = stringResource(R.string.settings_browser_open_default_settings),
                isLast = true,
                onTap = {
                    openingSettingsFailed = runCatching {
                        context.startActivity(
                            Intent(Settings.ACTION_MANAGE_DEFAULT_APPS_SETTINGS)
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                        )
                    }.isFailure
                },
            )
        }
        if (openingSettingsFailed) {
            Text(stringResource(R.string.settings_browser_open_settings_error), color = t.danger)
        }
        SettingsSection(
            label = stringResource(R.string.settings_browser_automation),
            footer = stringResource(R.string.settings_browser_desktop_note),
        ) {
            SettingsRow(
                label = stringResource(R.string.settings_browser_android_control),
                value = stringResource(if (feature.available) R.string.settings_cu_direct_title else R.string.settings_browser_unavailable),
                sub = stringResource(if (feature.available) R.string.settings_browser_android_supported else R.string.settings_browser_android_unavailable),
                chevron = false,
                isLast = true,
            )
        }
        if (feature.available) {
            Text(stringResource(R.string.settings_browser_saved_note), color = t.text3, fontSize = 13.sp)
            if (sessionRunning) {
                Text(stringResource(R.string.settings_browser_stop_to_change), color = t.text2, fontSize = 13.sp)
                Button(
                    onClick = { feature.stop(context) },
                    modifier = Modifier.fillMaxWidth().sizeIn(minHeight = 48.dp),
                ) { Text(stringResource(R.string.settings_cu_stop_now)) }
            }
            if (browsers.isEmpty()) {
                Text(stringResource(R.string.settings_browser_no_installed), color = t.text3)
            }
            browsers.forEach { browser ->
                val tier = browserControlTier(state, configuration, browser.packageName)
                val activeGrant = state.sessionState in setOf(ComputerUseSessionState.Active, ComputerUseSessionState.AwaitingApproval) &&
                    state.grants.any { it.packageName == browser.packageName }
                SettingsSection {
                    SettingsRow(
                        label = browser.label,
                        sub = stringResource(when {
                            sessionRunning && activeGrant -> R.string.settings_browser_session_authorized
                            browser.packageName in configuration.appSelections -> R.string.settings_browser_saved
                            else -> R.string.settings_cu_not_selected
                        }),
                        chevron = false,
                        isLast = tier == null,
                    ) {
                        LXToggle(
                            checked = tier != null,
                            enabled = !sessionRunning,
                            onCheckedChange = { checked ->
                                changePermission(browser.packageName, if (checked) ComputerUseTier.Read else null)
                            },
                        )
                    }
                    if (tier != null) {
                        FlowRow(
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp),
                        ) {
                            ComputerUseTier.entries.forEach { permission ->
                                FilterChip(
                                    selected = tier == permission,
                                    enabled = !sessionRunning,
                                    onClick = { changePermission(browser.packageName, permission) },
                                    label = { Text(stringResource(when (permission) {
                                        ComputerUseTier.Read -> R.string.settings_linux_read_only
                                        ComputerUseTier.Click -> R.string.computer_use_tier_click
                                        ComputerUseTier.Full -> R.string.settings_cu_tier_full
                                    })) },
                                    modifier = Modifier.sizeIn(minHeight = 48.dp),
                                )
                            }
                        }
                    }
                }
            }
            Button(
                onClick = onOpenComputerUse,
                modifier = Modifier.fillMaxWidth().sizeIn(minHeight = 48.dp),
            ) { Text(stringResource(R.string.settings_browser_manage_session)) }
        }
    }
}
