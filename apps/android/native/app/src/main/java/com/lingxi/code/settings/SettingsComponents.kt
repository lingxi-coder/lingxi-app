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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.mix
import com.lingxi.code.components.tint
import com.lingxi.code.theme.LXFont
import com.lingxi.code.theme.LingXiTheme

/**
 * Reusable settings building blocks — the Android port of the iOS
 * `SettingsComponents.swift` (Section / Row / RadioList / field helpers). Every
 * settings page is built from these so the grouped-card look, dividers, icon
 * tiles and chevrons stay 1:1 with the prototype.
 */

/** Grouped inset card with an optional uppercase label + footer note. */
@Composable
fun SettingsSection(
    modifier: Modifier = Modifier,
    label: String? = null,
    footer: String? = null,
    content: @Composable () -> Unit,
) {
    val t = LingXiTheme.palette
    Column(modifier = modifier.fillMaxWidth().padding(bottom = 22.dp)) {
        if (label != null) {
            Text(
                text = label.uppercase(),
                color = t.text4,
                fontSize = 11.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = 0.6.sp,
                modifier = Modifier.padding(horizontal = 4.dp).padding(bottom = 8.dp),
            )
        }
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(t.surface)
                .border(0.5.dp, t.border, RoundedCornerShape(12.dp)),
        ) { content() }
        if (footer != null) {
            Text(
                text = footer,
                color = t.text4,
                fontSize = 11.sp,
                lineHeight = 16.sp,
                modifier = Modifier.padding(horizontal = 4.dp).padding(top = 8.dp),
            )
        }
    }
}

/**
 * A settings list row. Mirrors the prototype's `<Row>`: optional leading icon
 * tile, title, sub, trailing value / accessory, chevron. State is hoisted — the
 * caller passes content and an [onTap].
 *
 * @param trailing a custom trailing accessory (toggle, slider) drawn before the
 *   value/chevron; defaults to nothing.
 */
@Composable
fun SettingsRow(
    label: String,
    modifier: Modifier = Modifier,
    icon: LXIconName? = null,
    iconColor: Color? = null,
    sub: String? = null,
    value: String? = null,
    valueColor: Color? = null,
    chevron: Boolean = true,
    danger: Boolean = false,
    isLast: Boolean = false,
    onTap: (() -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
) {
    val t = LingXiTheme.palette
    Column(modifier = modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier
                .fillMaxWidth()
                .then(if (onTap != null) Modifier.clickable(onClick = onTap) else Modifier)
                .padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            if (icon != null) {
                val c = iconColor ?: t.accent
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .size(28.dp)
                        .clip(RoundedCornerShape(7.dp))
                        .background(c.mix(t.surface, 0.16f))
                        .border(0.5.dp, c.tint(0.28f), RoundedCornerShape(7.dp)),
                ) {
                    LXIcon(name = icon, size = 14.dp, color = c, stroke = 1.8f)
                }
            }
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    text = label,
                    color = if (danger) t.danger else t.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (sub != null) {
                    Text(text = sub, color = t.text4, fontSize = 11.5f.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            if (value != null) {
                Text(text = value, color = valueColor ?: t.text3, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            trailing?.invoke()
            if (chevron && onTap != null) {
                LXIcon(name = LXIconName.ChevronR, size = 13.dp, color = t.text4, stroke = 1.7f)
            }
        }
        if (!isLast) {
            Box(Modifier.fillMaxWidth().size(0.5.dp).background(t.border))
        }
    }
}

/** One option of a [RadioList]. */
data class RadioOption(val value: String, val label: String, val sub: String? = null)

/** Single-select radio list (check on the right of the selected row). */
@Composable
fun RadioList(
    options: List<RadioOption>,
    selected: String,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(12.dp)),
    ) {
        options.forEachIndexed { i, o ->
            Column {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onSelect(o.value) }
                        .padding(horizontal = 14.dp, vertical = 12.dp),
                ) {
                    Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(o.label, color = t.text, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                        if (o.sub != null) Text(o.sub, color = t.text4, fontSize = 11.5f.sp)
                    }
                    if (selected == o.value) {
                        LXIcon(name = LXIconName.Check, size = 16.dp, color = t.accent, stroke = 2.5f)
                    }
                }
                if (i < options.size - 1) Box(Modifier.fillMaxWidth().size(0.5.dp).background(t.border))
            }
        }
    }
}

/** A small intro blurb above a page's sections (text3, tight leading). */
@Composable
fun Blurb(text: String, modifier: Modifier = Modifier) {
    Text(
        text = text,
        color = LingXiTheme.palette.text3,
        fontSize = 11.5f.sp,
        lineHeight = 16.sp,
        modifier = modifier.fillMaxWidth().padding(bottom = 14.dp),
    )
}

/** A dashed "+ add …" full-width button (used by the A7 provider list). */
@Composable
fun DashedAddButton(title: String, modifier: Modifier = Modifier, onClick: () -> Unit = {}) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .clickable(onClick = onClick)
            .border(1.dp, t.border, RoundedCornerShape(12.dp))
            .padding(13.dp),
    ) {
        LXIcon(name = LXIconName.Plus, size = 15.dp, color = t.text2, stroke = 2f)
        Text(title, color = t.text2, fontSize = 13.5f.sp, fontWeight = FontWeight.Medium)
    }
}

/** Standard monospaced text field used across the edit pages (A7/A8). */
@Composable
fun SettingsField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    mono: Boolean = true,
) {
    val t = LingXiTheme.palette
    Box(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(10.dp))
            .padding(horizontal = 12.dp, vertical = 11.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        val style = TextStyle(
            color = t.text,
            fontSize = 13.5f.sp,
            fontFamily = if (mono) LXFont.mono else LXFont.ui,
        )
        if (value.isEmpty() && placeholder.isNotEmpty()) {
            Text(placeholder, style = style.copy(color = t.text4))
        }
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            textStyle = style,
            singleLine = true,
            cursorBrush = SolidColor(t.accent),
            modifier = Modifier.fillMaxWidth(),
        )
    }
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

/** Bold field label above an editor field. */
@Composable
fun FieldLabel(text: String, modifier: Modifier = Modifier) {
    Text(
        text = text,
        color = LingXiTheme.palette.text3,
        fontSize = 12.sp,
        fontWeight = FontWeight.Medium,
        modifier = modifier.fillMaxWidth().padding(bottom = 6.dp),
    )
}

/** Small hint under a field (text4). */
@Composable
fun FieldHint(text: String, modifier: Modifier = Modifier) {
    Text(
        text = text,
        color = LingXiTheme.palette.text4,
        fontSize = 10.5f.sp,
        lineHeight = 15.sp,
        modifier = modifier.fillMaxWidth().padding(top = 4.dp, bottom = 14.dp),
    )
}
