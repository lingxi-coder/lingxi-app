package com.lingxi.code.settings

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.theme.LingXiTheme

/**
 * Dream mode, ported 1:1 from the iOS `DreamPage.swift`.
 *
 * A gradient-orb header (pulsing radial fill behind a moon glyph), the master
 * enable row (sub = last-run summary), a time-window radio (night / always /
 * custom), run-condition toggles (charging / Wi-Fi), the 5 stackable activities,
 * a compute-budget radio (low / medium / high), and a "last-night review"
 * timeline. All bound to the hoisted [SettingsStore]'s [com.lingxi.code.model.DreamConfig].
 */

private val DreamRose = Color(red = 0.809f, green = 0.4552f, blue = 0.8891f)  // oklch(70% 0.18 320)
private val DreamIndigo = Color(red = 0.1289f, green = 0.214f, blue = 0.6526f) // gradient inner-glow target

/** A Dream background activity (stackable). */
private data class DreamActivity(val key: String, val labelRes: Int, val subRes: Int)

private val DreamActivities = listOf(
    DreamActivity("reorganize", R.string.dream_activity_reorganize, R.string.dream_activity_reorganize_sub),
    DreamActivity("plan", R.string.dream_activity_plan, R.string.dream_activity_plan_sub),
    DreamActivity("recap", R.string.dream_activity_recap, R.string.dream_activity_recap_sub),
    DreamActivity("prefetch", R.string.dream_activity_prefetch, R.string.dream_activity_prefetch_sub),
    DreamActivity("polish", R.string.dream_activity_polish, R.string.dream_activity_polish_sub),
)

@Composable
fun DreamPage(
    state: SettingsUiState,
    store: SettingsStore,
) {
    val t = LingXiTheme.palette
    val dream = state.dream

    Column(Modifier.fillMaxWidth()) {
        // Gradient orb header -------------------------------------------------
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 22.dp),
        ) {
            DreamOrb()
            Text(
                stringResource(R.string.settings_dream_mode),
                color = t.text,
                fontSize = 19.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(top = 14.dp),
            )
            Text(
                stringResource(R.string.dream_description_blurb),
                color = t.text3,
                fontSize = 12.5f.sp,
                lineHeight = 17.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(top = 6.dp, start = 16.dp, end = 16.dp),
            )
        }

        // Master enable -------------------------------------------------------
        SettingsSection {
            SettingsRow(
                label = stringResource(R.string.dream_enable),
                sub = dream.lastRun,
                chevron = false,
                isLast = true,
                trailing = {
                    LXToggle(
                        checked = dream.enabled,
                        onCheckedChange = { store.setDream(dream.copy(enabled = it)) },
                    )
                },
            )
        }

        // Time window ---------------------------------------------------------
        SectionWithRadio(
            label = stringResource(R.string.dream_section_time_window),
            options = listOf(
                RadioOption("night", stringResource(R.string.dream_window_night), stringResource(R.string.dream_window_night_sub)),
                RadioOption("always", stringResource(R.string.dream_window_always), stringResource(R.string.dream_window_always_sub)),
                RadioOption("custom", stringResource(R.string.dream_window_custom), stringResource(R.string.dream_window_custom_sub)),
            ),
            selected = dream.window,
            onSelect = { store.setDream(dream.copy(window = it)) },
        )

        // Run conditions ------------------------------------------------------
        SettingsSection(label = stringResource(R.string.dream_section_conditions), footer = stringResource(R.string.dream_conditions_footer)) {
            SettingsRow(
                label = stringResource(R.string.dream_only_charging),
                chevron = false,
                trailing = {
                    LXToggle(
                        checked = dream.onCharging,
                        onCheckedChange = { store.setDream(dream.copy(onCharging = it)) },
                    )
                },
            )
            SettingsRow(
                label = stringResource(R.string.dream_only_wifi),
                chevron = false,
                isLast = true,
                trailing = {
                    LXToggle(
                        checked = dream.onWifi,
                        onCheckedChange = { store.setDream(dream.copy(onWifi = it)) },
                    )
                },
            )
        }

        // Allowed activities (stackable) --------------------------------------
        SettingsSection(label = stringResource(R.string.dream_section_activities), footer = stringResource(R.string.dream_activities_footer)) {
            DreamActivities.forEachIndexed { i, a ->
                val on = dream.activities[a.key] ?: false
                SettingsRow(
                    label = stringResource(a.labelRes),
                    sub = stringResource(a.subRes),
                    chevron = false,
                    isLast = i == DreamActivities.size - 1,
                    trailing = {
                        LXToggle(
                            checked = on,
                            onCheckedChange = { v ->
                                store.setDream(dream.copy(activities = dream.activities + (a.key to v)))
                            },
                        )
                    },
                )
            }
        }

        // Compute budget ------------------------------------------------------
        SectionWithRadio(
            label = stringResource(R.string.dream_section_compute_budget),
            footer = stringResource(R.string.dream_compute_budget_footer),
            options = listOf(
                RadioOption("low", stringResource(R.string.dream_budget_low), stringResource(R.string.dream_budget_low_sub)),
                RadioOption("medium", stringResource(R.string.dream_budget_medium), stringResource(R.string.dream_budget_medium_sub)),
                RadioOption("high", stringResource(R.string.dream_budget_high), stringResource(R.string.dream_budget_high_sub)),
            ),
            selected = dream.budget,
            onSelect = { store.setDream(dream.copy(budget = it)) },
        )

        // Last-night review ---------------------------------------------------
        SettingsSection(label = stringResource(R.string.dream_section_last_night)) {
            SettingsRow(label = stringResource(R.string.dream_recap_item_1), sub = stringResource(R.string.dream_recap_sub_1), onTap = {})
            SettingsRow(label = stringResource(R.string.dream_recap_item_2), sub = stringResource(R.string.dream_recap_sub_2), onTap = {})
            SettingsRow(label = stringResource(R.string.dream_recap_item_3), sub = stringResource(R.string.dream_recap_sub_3), isLast = true, onTap = {})
        }
    }
}

/** The pulsing gradient orb: a radial rose→indigo fill behind a moon glyph. */
@Composable
private fun DreamOrb() {
    val t = LingXiTheme.palette
    val transition = rememberInfiniteTransition(label = "dreamOrb")
    val pulse by transition.animateFloat(
        initialValue = 0.45f,
        targetValue = 0.8f,
        animationSpec = infiniteRepeatable(tween(3000), RepeatMode.Reverse),
        label = "orbPulse",
    )
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(76.dp)) {
        Box(
            modifier = Modifier
                .size(76.dp)
                .alpha(pulse)
                .clip(CircleShape)
                .background(
                    Brush.radialGradient(
                        colors = listOf(DreamRose, DreamIndigo),
                        center = Offset(0.3f * 76f, 0.3f * 76f),
                        radius = 70f,
                    ),
                ),
        )
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier.size(64.dp).clip(CircleShape).background(t.windowBg),
        ) {
            LXIcon(name = LXIconName.Dream, size = 32.dp, color = DreamRose, stroke = 1.6f)
        }
    }
}

/** A [SettingsSection]-styled label/footer wrapping a [RadioList]. */
@Composable
private fun SectionWithRadio(
    label: String,
    options: List<RadioOption>,
    selected: String,
    onSelect: (String) -> Unit,
    footer: String? = null,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(bottom = 22.dp)) {
        Text(
            text = label.uppercase(),
            color = t.text4,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            letterSpacing = 0.6.sp,
            modifier = Modifier.padding(horizontal = 4.dp).padding(bottom = 8.dp),
        )
        RadioList(options = options, selected = selected, onSelect = onSelect)
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
