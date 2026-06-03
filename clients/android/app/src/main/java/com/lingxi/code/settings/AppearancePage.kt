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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.theme.Accents
import com.lingxi.code.theme.AppearancePrefs
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.Density
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.ThemeMode
import kotlin.math.roundToInt
import kotlinx.coroutines.launch

/**
 * Appearance page — the live theme switchboard. Reads the resolved palette from
 * the [LingXiTheme] CompositionLocal and writes every choice straight to the
 * DataStore-backed [store]; because `MainActivity` collects the same store and
 * feeds it back into [LingXiTheme], theme / accent / density / text-size update
 * the whole app instantly and persist across launches (the Android analog of the
 * iOS `@AppStorage`-backed `AppState`).
 *
 * @param isDark the currently-resolved appearance (so the theme radio reflects
 *   the live state even when the mode is "system").
 * @param accentId the persisted accent id (drives the grid's selection ring).
 */
@Composable
fun AppearancePage(
    store: AppearanceStore,
    isDark: Boolean,
    accentId: String,
) {
    val t = LingXiTheme.palette
    val scope = rememberCoroutineScope()
    val prefs by store.prefs.collectAsState(initial = AppearancePrefs())

    Column(Modifier.fillMaxWidth()) {
        // 主题 ---------------------------------------------------------------
        SettingsSection(label = "主题") {
            RadioList(
                options = listOf(
                    RadioOption("light", "浅色", "暖白纸感 + 半透卡片"),
                    RadioOption("dark", "深色", "午夜紫 + 低饱和"),
                ),
                selected = if (isDark) "dark" else "light",
                onSelect = { v ->
                    scope.launch { store.setThemeMode(if (v == "dark") ThemeMode.Dark else ThemeMode.Light) }
                },
            )
        }

        // 强调色 -------------------------------------------------------------
        SettingsSection(label = "强调色") {
            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(12.dp))
                    .background(t.surface),
            ) {
                AccentGrid(
                    selectedId = accentId,
                    onSelect = { id -> scope.launch { store.setAccent(id) } },
                    modifier = Modifier.padding(14.dp),
                )
            }
        }

        // 密度 ---------------------------------------------------------------
        SettingsSection(label = "密度") {
            RadioList(
                options = listOf(
                    RadioOption(Density.Compact.raw, "紧凑", "一屏显示更多内容"),
                    RadioOption(Density.Comfortable.raw, "舒适", "默认，平衡"),
                    RadioOption(Density.Spacious.raw, "宽松", "更大间距，更易读"),
                ),
                selected = prefs.density.raw,
                onSelect = { v -> scope.launch { store.setDensity(Density.from(v)) } },
            )
        }

        // 字号 (live preview) -------------------------------------------------
        val fontSize = prefs.fontSize
        SettingsSection(label = "字号", footer = "当前 ${fontSize.roundToInt()}pt · 影响对话与列表正文") {
            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 16.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("A", color = t.text4, fontSize = 11.sp)
                    Slider(
                        value = fontSize,
                        onValueChange = { v -> scope.launch { store.setFontSize(v.roundToInt().toFloat()) } },
                        valueRange = 13f..19f,
                        steps = 5, // 13..19 inclusive → 7 stops → 5 interior steps
                        colors = SliderDefaults.colors(
                            thumbColor = t.accent,
                            activeTrackColor = t.accent,
                            inactiveTrackColor = t.surfaceActive,
                        ),
                        modifier = Modifier.weight(1f),
                    )
                    Text("A", color = t.text4, fontSize = 17.sp)
                }
                Text(
                    text = "“灵犀，帮我整理今天的会议要点，重点标出有 action item 的部分。”",
                    color = t.text,
                    fontSize = fontSize.sp,
                    lineHeight = (fontSize * 1.5f).sp,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(9.dp))
                        .background(t.windowBg)
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                )
            }
        }
    }
}

/** The 6-swatch accent grid — selection draws an inner ring + an outer halo. */
@Composable
private fun AccentGrid(
    selectedId: String,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    Row(modifier = modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Accents.all.forEach { a ->
            val selected = selectedId == a.id
            Column(
                modifier = Modifier.weight(1f).clickable { onSelect(a.id) },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                Box(contentAlignment = Alignment.Center, modifier = Modifier.size(34.dp)) {
                    // Outer halo ring (scaled out) on selection.
                    if (selected) {
                        Box(
                            Modifier
                                .size(34.dp)
                                .scale(1.18f)
                                .border(2.dp, a.color, CircleShape),
                        )
                    }
                    Box(
                        Modifier
                            .size(34.dp)
                            .clip(CircleShape)
                            .background(a.color)
                            .then(if (selected) Modifier.border(2.5.dp, t.windowBg, CircleShape) else Modifier),
                    )
                }
                Text(
                    a.name,
                    color = t.text3,
                    fontSize = 11.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}
