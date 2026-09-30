package com.lingxi.code.settings

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R

private data class PermissionModeChoice(val id: String, val label: String, val detail: String)

@Composable
fun PermissionModePage(
    selected: String,
    effective: String,
    error: String? = null,
    onSelect: (String) -> Unit,
) {
    val choices = listOf(
        PermissionModeChoice("default", stringResource(R.string.settings_permission_mode_default), stringResource(R.string.settings_permission_mode_default_detail)),
        PermissionModeChoice("acceptEdits", stringResource(R.string.settings_permission_mode_accept_edits), stringResource(R.string.settings_permission_mode_accept_edits_detail)),
        PermissionModeChoice("plan", stringResource(R.string.settings_permission_mode_plan), stringResource(R.string.settings_permission_mode_plan_detail)),
        PermissionModeChoice("auto", stringResource(R.string.settings_permission_mode_auto), stringResource(R.string.settings_permission_mode_auto_detail)),
        PermissionModeChoice("dontAsk", stringResource(R.string.settings_permission_mode_dont_ask), stringResource(R.string.settings_permission_mode_dont_ask_detail)),
        PermissionModeChoice("bypassPermissions", stringResource(R.string.settings_permission_mode_bypass), stringResource(R.string.settings_permission_mode_bypass_detail)),
    )
    var pending by remember { mutableStateOf<String?>(null) }
    SettingsSection(
        label = stringResource(R.string.settings_permission_mode_title),
        footer = stringResource(R.string.settings_permission_mode_footer),
    ) {
        RadioList(
            options = choices.map { RadioOption(it.id, it.label, it.detail) },
            selected = selected,
            onSelect = { id ->
                if (id == "dontAsk" || id == "bypassPermissions") pending = id else onSelect(id)
            },
        )
        if (!error.isNullOrBlank()) {
            Text(error)
        }
    }
    if (effective != selected) {
        Text(stringResource(R.string.settings_permission_mode_effective_fmt, effective))
    }
    pending?.let { mode ->
        AlertDialog(
            onDismissRequest = { pending = null },
            title = { Text(stringResource(R.string.settings_permission_mode_confirm_title)) },
            text = { Text(stringResource(R.string.settings_permission_mode_confirm_detail)) },
            confirmButton = {
                TextButton(onClick = { pending = null; onSelect(mode) }) {
                    Text(stringResource(R.string.settings_permission_mode_confirm))
                }
            },
            dismissButton = {
                TextButton(onClick = { pending = null }) {
                    Text(stringResource(R.string.settings_cancel))
                }
            },
        )
    }
}
