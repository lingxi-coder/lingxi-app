package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.clickable
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LocalControlLabel
import com.lingxi.code.theme.LXFont
import com.lingxi.code.theme.LingXiTheme

/** Shared native settings surfaces, with scalable rows and accessible controls. */
@Composable
fun SettingsSection(
    modifier: Modifier = Modifier, label: String? = null,
    footer: String? = null, content: @Composable () -> Unit,
) {
    Column(modifier.fillMaxWidth().padding(bottom = 24.dp)) {
        if (label != null) Text(label, color = MaterialTheme.colorScheme.primary,
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp))
        Surface(color = MaterialTheme.colorScheme.surfaceContainerLow,
            shape = MaterialTheme.shapes.large) {
            Column(Modifier.fillMaxWidth()) { content() }
        }
        if (footer != null) Text(footer, color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(horizontal = 16.dp).padding(top = 8.dp))
    }
}

@Composable
fun SettingsRow(
    label: String, modifier: Modifier = Modifier,
    icon: LXIconName? = null, iconColor: Color? = null,
    sub: String? = null, value: String? = null, valueColor: Color? = null,
    chevron: Boolean = true, danger: Boolean = false, isLast: Boolean = false,
    onTap: (() -> Unit)? = null, trailing: (@Composable () -> Unit)? = null,
) {
    val t = LingXiTheme.palette
    Column(modifier.fillMaxWidth()) {
        Surface(color = Color.Transparent, contentColor = t.text) {
            // Keep values with the text instead of squeezing the title between
            // a value and an accessory when Android's large fonts are enabled.
            ListItem(
                headlineContent = { Text(label, color = if (danger) t.danger else t.text) },
                supportingContent = if (sub != null || value != null) {{
                    Column {
                        sub?.let { Text(it, color = t.text2) }
                        value?.let { Text(it, color = valueColor ?: t.text2) }
                    }
                }} else null,
                leadingContent = icon?.let {{ LXIcon(it, color = iconColor ?: t.text2) }},
                trailingContent = if (trailing != null || (chevron && onTap != null)) {{
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        CompositionLocalProvider(LocalControlLabel provides label) { trailing?.invoke() }
                        if (chevron && onTap != null) LXIcon(LXIconName.ChevronR)
                    }
                }} else null,
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.fillMaxWidth().then(if (onTap != null) Modifier.clickable(onClick = onTap) else Modifier),
            )
        }
        if (!isLast) HorizontalDivider(color = t.border,
            modifier = Modifier.padding(start = if (icon != null) 56.dp else 16.dp, end = 16.dp))
    }
}

data class RadioOption(val value: String, val label: String, val sub: String? = null)

@Composable
fun RadioList(options: List<RadioOption>, selected: String, onSelect: (String) -> Unit,
    modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().selectableGroup()) {
        options.forEach { option ->
            Row(verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().selectable(
                    selected = selected == option.value, role = Role.RadioButton,
                    onClick = { onSelect(option.value) }).padding(horizontal = 16.dp, vertical = 8.dp)) {
                RadioButton(selected = selected == option.value, onClick = null)
                Spacer(Modifier.width(16.dp))
                Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                    Text(option.label, style = MaterialTheme.typography.bodyLarge)
                    option.sub?.let { Text(it, style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant) }
                }
            }
        }
    }
}

@Composable
fun Blurb(text: String, modifier: Modifier = Modifier) {
    Text(text, color = LingXiTheme.palette.text2, style = MaterialTheme.typography.bodyMedium,
        modifier = modifier.fillMaxWidth().padding(bottom = 16.dp))
}

@Composable
fun DashedAddButton(title: String, modifier: Modifier = Modifier, onClick: () -> Unit = {}) {
    OutlinedButton(onClick, modifier.fillMaxWidth().heightIn(min = 48.dp)) {
        LXIcon(LXIconName.Plus)
        Spacer(Modifier.width(8.dp))
        Text(title)
    }
}

@Composable
fun SettingsField(value: String, onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier, placeholder: String = "", mono: Boolean = true) {
    OutlinedTextField(value, onValueChange, modifier.fillMaxWidth(),
        placeholder = { Text(placeholder) }, singleLine = true,
        textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = if (mono) LXFont.mono else LXFont.ui))
}

/**
 * A self-contained toggle seeded with [seed]. Used for the prototype's
 * `.constant(...)`/local-only demo switches (auto-suggest, auto-start, per-tool
 * permissions) that have no store binding — they flip locally for fidelity but
 * are not persisted, matching the iOS shell.
 */
@Composable
fun LocalToggle(seed: Boolean, enabled: Boolean = true) {
    var on by remember { mutableStateOf(seed) }
    com.lingxi.code.components.LXToggle(checked = on, onCheckedChange = { on = it }, enabled = enabled)
}

/** Field labels and hints share Material's readable type scale. */
@Composable
fun FieldLabel(text: String, modifier: Modifier = Modifier) {
    Text(text, color = LingXiTheme.palette.text2, style = MaterialTheme.typography.labelLarge,
        modifier = modifier.fillMaxWidth().padding(bottom = 8.dp))
}

@Composable
fun FieldHint(text: String, modifier: Modifier = Modifier) {
    Text(text, color = LingXiTheme.palette.text2, style = MaterialTheme.typography.bodySmall,
        modifier = modifier.fillMaxWidth().padding(top = 4.dp, bottom = 16.dp))
}
