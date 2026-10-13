package com.lingxi.code.settings

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme

/** Show the current choice; reveal alternatives only on demand. */
@Composable
internal fun VoiceChoiceRow(label: String, options: List<RadioOption>, selected: String, onSelect: (String) -> Unit) {
    val t = LingXiTheme.palette
    var expanded by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxWidth()) {
        SettingsRow(label = label, value = options.firstOrNull { it.value == selected }?.label ?: stringResource(R.string.audio_guide_configuration_title),
            onTap = { expanded = true })
        DropdownMenu(expanded, onDismissRequest = { expanded = false }, modifier = Modifier.heightIn(max = 420.dp)) {
            options.forEach { option ->
                DropdownMenuItem(text = {
                    Column {
                        Text(option.label, color = t.text)
                        option.sub?.let { Text(it, color = t.text3) }
                    }
                }, onClick = { onSelect(option.value); expanded = false })
            }
        }
    }
}

@Composable
internal fun VoiceSettingsDisclosure(title: String, initiallyOpen: Boolean = false, content: @Composable () -> Unit) {
    var expanded by remember { mutableStateOf(initiallyOpen) }
    LaunchedEffect(initiallyOpen) { if (initiallyOpen) expanded = true }
    Column {
        SettingsSection {
            SettingsRow(label = title, value = stringResource(if (expanded) R.string.audio_settings_collapse else R.string.audio_settings_expand),
                isLast = true, chevron = false, onTap = { expanded = !expanded })
        }
        if (expanded) content()
    }
}
