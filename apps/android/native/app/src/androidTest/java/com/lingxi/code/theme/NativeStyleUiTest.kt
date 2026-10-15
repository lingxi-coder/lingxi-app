package com.lingxi.code.theme

import android.graphics.Bitmap
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.components.LXToggle
import com.lingxi.code.settings.*
import com.lingxi.code.conversation.*
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import com.lingxi.code.onboarding.SetupWizardOverlay
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class NativeStyleUiTest {
    @get:Rule val rule = createComposeRule()
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext

    private fun capture(name: String) {
        rule.waitForIdle()
        val file = File(context.getExternalFilesDir("native-ui-audit"), "$name.png")
        file.parentFile?.mkdirs()
        file.outputStream().use { rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    @Test fun everySettingsDestinationInBothAppearances() {
        val routes = (settingsGroupEntries(true).map { it.route } + SettingsRoutes.MAIN + SettingsRoutes.ACCOUNT + SettingsRoutes.CRON).distinct()
        File(context.getExternalFilesDir("native-ui-audit"), "routes.tsv").apply {
            parentFile?.mkdirs()
            writeText(routes.mapIndexed { index, value -> "$index\t$value" }.joinToString("\n"))
        }
        val route = mutableStateOf(routes.first())
        val dark = mutableStateOf(false)
        val appearance = AppearanceStore(context)
        rule.setContent {
            LingXiTheme(darkTheme = dark.value) {
                key(route.value, dark.value) {
                    SettingsHost(appearanceStore = appearance, isDark = dark.value,
                        accentId = Accents.DEFAULT_ID, initialRoute = route.value)
                }
            }
        }
        for (night in listOf(false, true)) for ((index, target) in routes.withIndex()) {
            rule.runOnIdle { dark.value = night; route.value = target }
            capture("settings-${if (night) "dark" else "light"}-$index")
            rule.onRoot().assertIsDisplayed()
        }
    }

    @Test fun largeTextRowsWrapAndNativeControlsRemainOperable() {
        val checked = mutableStateOf(false)
        val selected = mutableStateOf("a")
        val label = "A long settings label that must remain readable at the largest font size"
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                val density = LocalDensity.current
                CompositionLocalProvider(LocalDensity provides Density(density.density, 2f)) {
                    Surface {
                        Column(Modifier.fillMaxWidth()) {
                            SettingsRow(label, sub = "Supporting text also wraps instead of disappearing", chevron = false,
                                trailing = { LXToggle(checked.value, { checked.value = it }) })
                            RadioList(listOf(RadioOption("a", "First option"), RadioOption("b", "Second option")),
                                selected.value, { selected.value = it })
                        }
                    }
                }
            }
        }
        rule.onNodeWithContentDescription(label).assertIsOff().performClick().assertIsOn()
        val layouts = mutableListOf<TextLayoutResult>()
        rule.onNodeWithText(label).performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult) { it(layouts) }
        assertTrue(layouts.isNotEmpty())
        assertFalse(layouts.single().hasVisualOverflow)
        rule.onNodeWithText("Second option").performClick().assertIsSelected()
        assertTrue(checked.value)
        capture("settings-large-text")
    }

    @Test fun chatAndOnboardingInBothAppearances() {
        val dark = mutableStateOf(false)
        val onboarding = mutableStateOf(false)
        rule.setContent {
            LingXiTheme(darkTheme = dark.value) {
                Surface(Modifier.fillMaxSize()) {
                    if (onboarding.value) SetupWizardOverlay(true, "Lingxi", "", false, "", "", { _, _, _, _, _ -> })
                    else ChatScreen(ChatState(session = SessionRef("native-audit", "Conversation"), messages = listOf(
                            com.lingxi.code.model.Message(com.lingxi.code.model.Role.User, "Help me review the project and plan the next steps."),
                            com.lingxi.code.model.Message(com.lingxi.code.model.Role.Ai,
                                "## Project review\nI’ll check the current implementation, verify the important flows, and summarize the next steps.\n\n- Review the main screens\n- Check light and dark appearance\n- Verify accessibility and keyboard behavior")),
                        model = EngineModelCatalog.pending), onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = dark.value)
                }
            }
        }
        for (night in listOf(false, true)) {
            rule.runOnIdle { dark.value = night; onboarding.value = false }
            capture("chat-${if (night) "dark" else "light"}")
            rule.runOnIdle { onboarding.value = true }
            capture("onboarding-${if (night) "dark" else "light"}")
            rule.onNodeWithText(context.getString(R.string.onboarding_cta_start)).performClick()
            capture("onboarding-assistant-${if (night) "dark" else "light"}")
            rule.onNodeWithText(context.getString(R.string.onboarding_cta_continue)).performClick()
            rule.onNode(hasSetTextAction()).performTextInput("Tester")
            androidx.test.espresso.Espresso.closeSoftKeyboard()
            capture("onboarding-user-${if (night) "dark" else "light"}")
            rule.onNodeWithText(context.getString(R.string.onboarding_cta_continue)).performClick()
            capture("onboarding-voice-${if (night) "dark" else "light"}")
            rule.onNodeWithText(context.getString(R.string.onboarding_cta_continue)).performClick()
            capture("onboarding-ready-${if (night) "dark" else "light"}")
        }
    }
    @Test fun primaryPagesInBothAppearances() {
        val dark = mutableStateOf(false)
        val apps = mutableStateOf(false)
        val localApps = com.lingxi.code.apps.LocalApps.get(context)
        rule.setContent {
            LingXiTheme(darkTheme = dark.value) {
                Surface(Modifier.fillMaxSize()) {
                    if (apps.value) com.lingxi.code.apps.AppsPage(localApps, {}, {}, {})
                    else com.lingxi.code.project.ProjectsPage(emptyList(), emptyMap(), null, {}, {}, {}, {})
                }
            }
        }
        for (night in listOf(false, true)) {
            rule.runOnIdle { dark.value = night; apps.value = false }
            capture("projects-${if (night) "dark" else "light"}")
            rule.runOnIdle { apps.value = true }
            capture("apps-${if (night) "dark" else "light"}")
        }
    }

    @Test fun newChatEmptyStateFitsShortViewport() {
        val draft = mutableStateOf("")
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Surface(Modifier.fillMaxWidth().height(460.dp)) {
                    ChatScreen(ChatState(session = SessionRef("new", "New conversation"), isNew = true, messages = emptyList(),
                        model = EngineModelCatalog.pending), onSend = {}, onNewChat = {}, onSelectModel = {},
                        isDark = false, draft = draft.value, onDraftChange = { draft.value = it })
                }
            }
        }
        val title = rule.onNodeWithText(context.getString(R.string.chat_start_new_conversation))
        title.assertIsDisplayed()
        val input = rule.onNode(hasSetTextAction())
        assertTrue(title.getUnclippedBoundsInRoot().bottom <= input.getUnclippedBoundsInRoot().top)
        input.performTextInput("Draft remains editable")
        input.assertTextContains("Draft remains editable")
        capture("chat-short-viewport")
    }

}
