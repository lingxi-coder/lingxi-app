package com.lingxi.code

import android.view.WindowManager
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.components.UiTags
import com.lingxi.code.drawer.DrawerSection
import com.lingxi.code.theme.AppearanceStore
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.rules.TestRule
import org.junit.runner.RunWith
import org.junit.runner.Description
import org.junit.runners.model.Statement

/**
 * Instrumented Compose UI tests for the app shell — drawer tab switching,
 * settings push-navigation + system back, and the live theme / accent toggles.
 *
 * These exercise the real [MainActivity] composition (drawer + conversation +
 * settings NavHost + DataStore-backed theme), so they REQUIRE a connected
 * device or emulator (`./gradlew :app:connectedDebugAndroidTest`). They are NOT
 * part of the headless CI gate (`assembleDebug` + `testDebugUnitTest`), which is
 * why the JVM unit tests cover the data/color invariants device-free. See the
 * module README.
 *
 * Anchors: icon-only affordances use [UiTags]; everything else is found by its
 * visible Chinese label, exactly as a user would read it.
 */
@RunWith(AndroidJUnit4::class)
class AppFlowUiTest {

    private val composeRule = createAndroidComposeRule<MainActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain
        .outerRule(SkipOnboardingRule())
        .around(composeRule)

    private val rule get() = composeRule

    @Suppress("DEPRECATION")
    @Test
    fun mainWindow_resizesContentForTheIme() {
        val adjustMode = rule.activity.window.attributes.softInputMode and
            WindowManager.LayoutParams.SOFT_INPUT_MASK_ADJUST

        assertEquals(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE, adjustMode)
    }

    @Test
    fun conversation_doesNotShowPrototypeWorkflowTabs() {
        rule.onNodeWithText("理解需求").assertDoesNotExist()
        rule.onNodeWithText("检索 Claude iOS").assertDoesNotExist()
        rule.onNodeWithText("抽屉/侧栏组件").assertDoesNotExist()
        rule.onNodeWithText("语音模式集成").assertDoesNotExist()
    }

    /** Open the 对话/项目/定时 drawer via the conversation top-bar hamburger. */
    private fun openDrawer() {
        rule.onNodeWithTag(UiTags.OPEN_DRAWER).performClick()
        rule.waitForIdle()
    }

    // --- 1. drawer tab switch ---------------------------------------------

    @Test
    fun drawer_switchesBetweenChatsProjectsAndCrons() {
        openDrawer()

        rule.onNodeWithText("知识库").assertDoesNotExist()
        rule.onNodeWithText("记忆").assertDoesNotExist()

        // Project is backed by the real filesystem repository. A clean install
        // starts empty but always exposes the create/import action.
        rule.onNodeWithTag(UiTags.drawerTab(DrawerSection.Projects.key)).performClick()
        rule.waitForIdle()
        rule.onNodeWithText("新建或导入项目").assertIsDisplayed()

        // Cron is also a production collection: clean install is explicitly
        // empty rather than populated with prototype rows.
        rule.onNodeWithTag(UiTags.drawerTab(DrawerSection.Crons.key)).performClick()
        rule.waitForIdle()
        rule.onNodeWithText("暂无定时任务").assertIsDisplayed()

        // Back to 对话 → the real engine catalog surface is still reachable.
        rule.onNodeWithTag(UiTags.drawerTab(DrawerSection.Chats.key)).performClick()
        rule.waitForIdle()
        rule.onNodeWithTag(UiTags.drawerTab(DrawerSection.Chats.key)).assertIsDisplayed()
    }

    // --- 2. settings push-nav + system back -------------------------------

    @Test
    fun settings_pushNavToAppearance_andSystemBackReturnsToRoot() {
        openDrawer()

        // Account row opens the settings surface.
        rule.onNodeWithText("Yuxin Yang").performClick()
        rule.waitForIdle()
        // Root settings title.
        rule.onNodeWithText("设置").assertIsDisplayed()
        rule.onNodeWithText("知识库").assertDoesNotExist()
        rule.onNodeWithText("记忆").assertDoesNotExist()

        // Push to 外观 (Appearance).
        rule.onNodeWithText("外观").performScrollTo().performClick()
        rule.waitForIdle()
        // The Appearance page shows its 主题 section + theme radios.
        rule.onNodeWithText("主题").assertIsDisplayed()
        rule.onNodeWithText("强调色").assertIsDisplayed()

        // System back pops one page → back at the settings root.
        androidx.test.platform.app.InstrumentationRegistry
            .getInstrumentation().runOnMainSync { rule.activity.onBackPressedDispatcher.onBackPressed() }
        rule.waitForIdle()
        rule.onNodeWithText("设置").assertIsDisplayed()
    }

    // --- 3. theme + accent toggle -----------------------------------------

    @Test
    fun appearance_themeRadioAndAccentSwatchApply() {
        openDrawer()
        rule.onNodeWithText("Yuxin Yang").performClick()
        rule.waitForIdle()
        rule.onNodeWithText("外观").performScrollTo().performClick()
        rule.waitForIdle()

        // Toggle theme to 浅色 (light); the radio + live preview remain present.
        rule.onNodeWithText("浅色").performScrollTo().performClick()
        rule.waitForIdle()
        rule.onNodeWithText("浅色").assertIsDisplayed()

        // Pick a non-default accent swatch (玫红); the grid stays rendered.
        rule.onNodeWithText("玫红").performScrollTo().performClick()
        rule.waitForIdle()
        rule.onNodeWithText("玫红").assertIsDisplayed()

        // Flip back to 深色 (dark) — the toggle round-trips without crashing.
        rule.onNodeWithText("深色").performScrollTo().performClick()
        rule.waitForIdle()
        rule.onNodeWithText("深色").assertIsDisplayed()

        // The page is still the Appearance page after the toggles.
        rule.onAllNodesWithText("密度")[0].performScrollTo().assertIsDisplayed()
    }
}

/**
 * App-flow tests target the conversation/drawer shell, not the first-run wizard.
 * Prepare DataStore before [MainActivity] launches, then restore the prior value
 * afterward. Notification permission remains granted on the debug test install:
 * revoking it while instrumentation is attached makes Android kill the target
 * process and abort the remaining test class.
 */
private class SkipOnboardingRule : TestRule {
    override fun apply(base: Statement, description: Description): Statement =
        object : Statement() {
            override fun evaluate() {
                val instrumentation = androidx.test.platform.app.InstrumentationRegistry
                    .getInstrumentation()
                val context = instrumentation.targetContext
                val store = AppearanceStore(context)
                val wasDone = runBlocking { store.prefs.first().setupDone }
                val notificationPermission = android.Manifest.permission.POST_NOTIFICATIONS
                val hadNotificationPermission =
                    android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.TIRAMISU ||
                        androidx.core.content.ContextCompat.checkSelfPermission(
                            context,
                            notificationPermission,
                        ) == android.content.pm.PackageManager.PERMISSION_GRANTED
                if (!wasDone) runBlocking { store.setSetupDone(true) }
                if (!hadNotificationPermission) {
                    instrumentation.uiAutomation.grantRuntimePermission(
                        context.packageName,
                        notificationPermission,
                    )
                }
                try {
                    base.evaluate()
                } finally {
                    if (!wasDone) runBlocking { store.setSetupDone(false) }
                }
            }
        }
}
