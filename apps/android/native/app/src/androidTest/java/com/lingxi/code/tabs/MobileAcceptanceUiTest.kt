package com.lingxi.code.tabs

import android.content.res.Configuration
import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import java.util.Locale
import kotlinx.coroutines.flow.MutableStateFlow
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertTrue
import com.lingxi.code.testing.finishComposeFixture
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Production shell resources/layout/semantics under the spec's locale and text-size matrix. */
@RunWith(AndroidJUnit4::class)
class MobileAcceptanceUiTest {
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()
    private data class Case(val locale: String, val fontScale: Float, val width: Int, val dark: Boolean)
    private val labels = listOf(R.string.tabs_chats, R.string.remote_bots, R.string.teams_title, R.string.settings_title_main)

    @Test fun fiveLocalesKeepAllTabLabelsReadableAtDefaultAndLargestText() {
        var current by mutableStateOf(Case("en", 1f, 320, false))
        val base = rule.activity
        rule.setContent {
            val config = Configuration(base.resources.configuration).apply {
                setLocale(Locale.forLanguageTag(current.locale))
                fontScale = current.fontScale
            }
            val localized = base.createConfigurationContext(config)
            CompositionLocalProvider(LocalContext provides localized, LocalConfiguration provides config,
                // Integral pixel density avoids subpixel height rounding looking like clipped text.
                LocalDensity provides Density(1f, current.fontScale)) {
                LingXiTheme(darkTheme = current.dark) {
                    key(current) {
                        Box(Modifier.requiredSize(current.width.dp, 900.dp)) {
                            MainTabShell(rememberTabShellState(), false,
                                chats = { Text("fixture") }, settings = { Text("fixture") },
                                bots = { Text("fixture") }, teams = { Text("fixture") },
                                botsBadge = MutableStateFlow(0), teamsBadge = MutableStateFlow(0))
                        }
                    }
                }
            }
        }
        val results = JSONArray()
        val report = File(base.getExternalFilesDir("mobile-port-acceptance"), "configuration-matrix.json")
        report.parentFile?.mkdirs()
        val failures = mutableListOf<String>()
        for (locale in listOf("zh-Hans", "zh-Hant", "en", "ja", "ko")) {
            for (scale in listOf(1f, 2f)) for (width in listOf(320, 900)) for (dark in listOf(false, true)) {
                val sample = Case(locale, scale, width, dark)
                rule.runOnIdle { current = sample }
                val config = Configuration(base.resources.configuration).apply { setLocale(Locale.forLanguageTag(locale)) }
                val context = base.createConfigurationContext(config)
                val overflows = mutableListOf<String>()
                val layoutReports = JSONArray()
                for (id in labels) {
                    val label = context.getString(id)
                    val layouts = mutableListOf<TextLayoutResult>()
                    rule.onNodeWithText(label, useUnmergedTree = true).assertIsDisplayed()
                        .performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
                    // MultiParagraph retains available width when Text shrinks to intrinsic width.
                    // Check actual line extents and ellipsis, rather than didOverflowWidth.
                    if (layouts.isEmpty() || layouts.any { layout ->
                        layout.multiParagraph.height > layout.size.height + 1 ||
                            (0 until layout.lineCount).any { line ->
                                layout.isLineEllipsized(line) ||
                                    layout.getLineRight(line) - layout.getLineLeft(line) > layout.size.width + 1
                            }
                    }) overflows += label
                    for (layout in layouts) layoutReports.put(JSONObject().put("label", label)
                        .put("width", layout.size.width).put("height", layout.size.height)
                        .put("paragraphWidth", layout.multiParagraph.width).put("paragraphHeight", layout.multiParagraph.height)
                        .put("overflowWidth", layout.didOverflowWidth).put("overflowHeight", layout.didOverflowHeight)
                        .put("ellipsized", (0 until layout.lineCount).any { layout.isLineEllipsized(it) }))
                }
                results.put(JSONObject().put("locale", locale).put("fontScale", scale).put("widthDp", width)
                    .put("dark", dark).put("overflowLabels", JSONArray(overflows)).put("textLayouts", layoutReports))
                report.writeText(results.toString(2))
                if (overflows.isNotEmpty()) failures += "$sample: $overflows"
                if (!dark && (locale == "en" || locale == "ja" && scale == 2f && width == 320)) {
                    File(base.getExternalFilesDir("mobile-port-acceptance"), "matrix-$locale-$scale-$width.png").apply {
                        parentFile?.mkdirs()
                        outputStream().use { rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) }
                    }
                }
            }
        }
        assertTrue(failures.joinToString("\n"), failures.isEmpty())
    }

    @Test fun navigationExposesSelectionAndFullBadgeValuesToAccessibility() {
        rule.setContent {
            LingXiTheme {
                MainTabShell(rememberTabShellState(), false,
                    chats = { Text("fixture") }, settings = { Text("fixture") },
                    bots = { Text("fixture") }, teams = { Text("fixture") },
                    botsBadge = MutableStateFlow(125), teamsBadge = MutableStateFlow(3))
            }
        }
        rule.onNodeWithContentDescription(rule.activity.getString(R.string.tabs_navigation_a11y)).assertExists()
        rule.onNodeWithTag(UiTags.tab("chats")).assertIsSelected()
        rule.onNodeWithTag(UiTags.tab("bots")).performClick().assertIsSelected()
        rule.onNodeWithTag(UiTags.tab("bots")).assert(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, rule.activity.getString(R.string.remote_bot_waiting_fmt, 125)))
        rule.onNodeWithTag(UiTags.tab("teams")).assert(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, rule.activity.getString(R.string.tabs_badge_unread_fmt, 3)))
    }
}
