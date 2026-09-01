package com.lingxi.code.settings

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R

private data class TypeScriptLspChoice(val id: String, val label: String, val detail: String)

@Composable
fun TypeScriptLspModePage(
    selected: String,
    effective: String,
    available: Boolean,
    error: String? = null,
    onSelect: (String) -> Unit,
) {
    val choices = listOf(
        TypeScriptLspChoice("auto", stringResource(R.string.settings_typescript_lsp_auto), stringResource(R.string.settings_typescript_lsp_auto_detail)),
        TypeScriptLspChoice("off", stringResource(R.string.settings_typescript_lsp_off), stringResource(R.string.settings_typescript_lsp_off_detail)),
        TypeScriptLspChoice("on", stringResource(R.string.settings_typescript_lsp_on), stringResource(R.string.settings_typescript_lsp_on_detail)),
    )
    SettingsSection(
        label = stringResource(R.string.settings_typescript_lsp_title),
        footer = stringResource(R.string.settings_typescript_lsp_footer),
    ) {
        RadioList(
            options = choices.map { RadioOption(it.id, it.label, it.detail) },
            selected = selected,
            onSelect = onSelect,
        )
        if (!error.isNullOrBlank()) Text(error)
    }
    if (!available && selected != "off") {
        Text(stringResource(R.string.settings_typescript_lsp_unavailable))
    } else if (effective != selected) {
        Text(stringResource(R.string.settings_typescript_lsp_effective_fmt, effective))
    }
}
