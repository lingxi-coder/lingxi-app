package com.lingxi.code.settings

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.isDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalTabReselects
import com.lingxi.code.tabs.LocalWideLayout
import com.lingxi.code.tabs.SettingsJump
import com.lingxi.code.tabs.TabReselect
import com.lingxi.code.theme.Accents
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.flow.MutableSharedFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import com.lingxi.code.testing.finishComposeFixture
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The Settings tab body ([SettingsHost]) the way the shell drives it (SPEC-mobile-team-bot-ui section 5): no
 * close or "Done" anywhere, the back arrow on pages only, jumps that replace the stack, a jump's origin honoured by
 * the arrow as well as by the system Back, depth reported for the bar, and the wide page list.
 *
 * The harness plays the shell's part: it holds the pending jump and the origin, clears the jump once the host took it,
 * and records every depth and every "leave for this tab" the host reports. The pure rules behind Back are covered on
 * the JVM by `SettingsBackActionTest`; this class checks that the host applies them.
 */
@RunWith(AndroidJUnit4::class)
class SettingsTabUiTest {
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private var jump by mutableStateOf<SettingsJump?>(null)
    private var origin by mutableStateOf<AppTab?>(null)
    private val depths = mutableListOf(-1)
    private val handled = mutableListOf<Long>()
    private val leftTo = mutableListOf<AppTab>()
    private val reselects = MutableSharedFlow<TabReselect>(extraBufferCapacity = 1)
    private var jumps = 0L

    private fun launch(wide: Boolean = false, height: Dp = 800.dp) {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                // Fit the synthetic viewport in both physical orientations. The fixture activity's
                // decor already consumes system bars; keep their pixel insets out of this dp model.
                CompositionLocalProvider(LocalWideLayout provides wide, LocalTabReselects provides reselects,
                    LocalDensity provides Density(0.5f, 1f)) {
                    Box(Modifier.requiredSize(if (wide) 900.dp else 400.dp, height)
                        .consumeWindowInsets(WindowInsets.safeDrawing)) {
                        SettingsHost(
                            appearanceStore = remember { AppearanceStore(rule.activity.applicationContext) },
                            isDark = false,
                            accentId = Accents.DEFAULT_ID,
                            jump = jump,
                            onJumpHandled = { id -> handled += id; if (jump?.id == id) jump = null },
                            origin = origin,
                            onOriginBack = { tab -> leftTo += tab; origin = null },
                            onDepthChanged = { depths += it },
                        )
                    }
                }
            }
        }
        rule.waitForIdle()
    }

    private fun text(id: Int) = rule.activity.getString(id)

    /** What `TabShellState.openSettings` does: remember the origin and publish the request. */
    private fun jumpTo(route: String, from: AppTab? = null) {
        rule.runOnIdle {
            origin = from
            jump = SettingsJump(++jumps, route, from)
        }
        rule.waitForIdle()
    }

    private fun pressSystemBack() {
        rule.runOnUiThread { rule.activity.onBackPressedDispatcher.onBackPressed() }
        rule.waitForIdle()
    }

    private fun pressArrow() {
        rule.onNodeWithTag(SettingsTags.BACK).performClick()
        rule.waitForIdle()
    }

    private fun openGeneralGroup() {
        rule.onNodeWithTag(UiTags.settingsGroup("general")).performScrollTo().performClick()
        rule.waitForIdle()
    }

    private fun openAppearanceFromTheGroup() {
        rule.onNodeWithTag(UiTags.settingsPage(SettingsRoutes.APPEARANCE)).performScrollTo().performClick()
        rule.waitForIdle()
    }

    private fun assertOnTheRoot() {
        rule.onNodeWithTag(UiTags.SETTINGS_SEARCH).assertIsDisplayed()
        rule.onNodeWithTag(SettingsTags.BACK).assertDoesNotExist()
    }

    private val topBarButtons get() = hasClickAction() and hasAnyAncestor(hasTestTag(SettingsTags.TOP_BAR))

    // --- chrome -----------------------------------------------------------------------------------------------------

    @Test
    fun theRootHasNoCloseNoDoneAndNoBackArrow() {
        launch()

        assertOnTheRoot()
        rule.onNodeWithTag(SettingsTags.TOP_BAR).assertIsDisplayed()
        // Nothing in the top bar can be pressed: the tab is left through the bar, never through a close action.
        rule.onNode(topBarButtons).assertDoesNotExist()
        assertEquals(0, depths.last())
        assertTrue(leftTo.isEmpty())
    }

    @Test
    fun aPageHasABackArrowAndNothingElseToPress() {
        launch()

        openGeneralGroup()

        rule.onNodeWithTag(SettingsTags.BACK).assertIsDisplayed()
        rule.onAllNodes(topBarButtons).assertCountEquals(1)
        assertEquals(1, depths.last())

        pressArrow()
        assertOnTheRoot()
        assertEquals(0, depths.last())
        assertTrue("a plain page is not a jump: nothing to leave for", leftTo.isEmpty())
    }

    @Test
    fun depthFollowsThePagesAboveTheRoot() {
        launch()

        openGeneralGroup()
        assertEquals(1, depths.last())
        openAppearanceFromTheGroup()
        assertEquals(2, depths.last())

        pressSystemBack()
        assertEquals(1, depths.last())
        pressSystemBack()
        assertEquals(0, depths.last())
    }

    // --- jumps ------------------------------------------------------------------------------------------------------

    @Test
    fun aJumpReplacesTheStackAndIsHandledOnce() {
        launch()
        openGeneralGroup()
        openAppearanceFromTheGroup()
        assertEquals(2, depths.last())

        jumpTo(SettingsRoutes.ACCOUNT)

        // [root, account]: the two pages that were open are gone, not buried.
        assertEquals(1, depths.last())
        assertEquals(listOf(jumps), handled)
        pressSystemBack()
        assertOnTheRoot()
    }

    @Test
    fun aJumpToTheRootPopsEverything() {
        launch()
        openGeneralGroup()
        openAppearanceFromTheGroup()

        jumpTo(SettingsRoutes.MAIN)

        assertOnTheRoot()
        assertEquals(0, depths.last())
        assertEquals(listOf(jumps), handled)
    }

    @Test
    fun aPageThisBuildDoesNotHaveLeavesTheStackAlone() {
        launch()

        jumpTo("settings/this-page-does-not-exist")

        assertOnTheRoot()
        assertEquals(0, depths.last())
        assertEquals("the request is still answered, or it would be replayed", listOf(jumps), handled)
    }

    // --- origin -----------------------------------------------------------------------------------------------------

    @Test
    fun theArrowOnAJumpedPageLeavesForTheOriginWithTheStackCleared() {
        launch()
        jumpTo(SettingsRoutes.PERMISSION_MODE, from = AppTab.Chats)
        assertEquals(1, depths.last())

        pressArrow()

        assertEquals(listOf(AppTab.Chats), leftTo)
        // Popped to the root before leaving, so coming back to Settings starts there.
        assertEquals(0, depths.last())
        assertOnTheRoot()
    }

    @Test
    fun theSystemBackOnAJumpedPageLeavesForTheOriginToo() {
        launch()
        jumpTo(SettingsRoutes.ACCOUNT, from = AppTab.Teams)

        pressSystemBack()

        assertEquals(listOf(AppTab.Teams), leftTo)
        assertEquals(0, depths.last())
    }

    @Test
    fun deeperPagesPopBeforeTheOriginIsLeft() {
        launch()
        jumpTo(SettingsRoutes.group("general"), from = AppTab.Bots)
        openAppearanceFromTheGroup()
        assertEquals(2, depths.last())

        pressArrow()
        assertTrue("one page up is not the end of the jump", leftTo.isEmpty())
        assertEquals(1, depths.last())

        pressSystemBack()
        assertEquals(listOf(AppTab.Bots), leftTo)
        assertEquals(0, depths.last())
    }

    @Test
    fun theRootAfterAJumpToItLeavesForTheOrigin() {
        launch()
        jumpTo(SettingsRoutes.MAIN, from = AppTab.Bots)

        pressSystemBack()

        assertEquals(listOf(AppTab.Bots), leftTo)
    }

    @Test
    fun aTabTapForgetsTheOrigin() {
        launch()
        jumpTo(SettingsRoutes.ACCOUNT, from = AppTab.Chats)
        // The user tapped the Settings tab: the shell no longer passes the origin.
        rule.runOnIdle { origin = null }
        rule.waitForIdle()

        pressArrow()

        assertTrue(leftTo.isEmpty())
        assertOnTheRoot()
    }

    @Test
    fun aSecondJumpReplacesTheOriginToo() {
        launch()
        jumpTo(SettingsRoutes.ACCOUNT, from = AppTab.Chats)
        jumpTo(SettingsRoutes.PERMISSION_MODE, from = AppTab.Teams)

        pressArrow()

        assertEquals(listOf(AppTab.Teams), leftTo)
    }

    // --- the tab bar's second tap -----------------------------------------------------------------------------------

    @Test
    fun tappingTheTabAgainPopsToTheRoot() {
        launch()
        openGeneralGroup()
        openAppearanceFromTheGroup()

        rule.runOnIdle { reselects.tryEmit(TabReselect(AppTab.Settings, depth = 2)) }
        rule.waitForIdle()

        assertOnTheRoot()
        assertEquals(0, depths.last())
    }

    @Test
    fun tappingTheTabAgainOnTheRootScrollsItToTheTop() {
        launch(height = 300.dp)
        // The last group is below the fold of a 300 dp window.
        rule.onNodeWithTag(UiTags.settingsGroup("about")).performScrollTo().assertIsDisplayed()

        rule.runOnIdle { reselects.tryEmit(TabReselect(AppTab.Settings, depth = 0)) }
        rule.waitUntil(5_000) { rule.onNodeWithTag(UiTags.SETTINGS_SEARCH).isDisplayed() }

        rule.onNodeWithTag(UiTags.SETTINGS_SEARCH).assertIsDisplayed()
    }

    // --- wide -------------------------------------------------------------------------------------------------------

    @Test
    fun phone_hasNoPageList() {
        launch()

        rule.onNodeWithTag(SettingsTags.DESKTOP_NAV).assertDoesNotExist()
    }

    @Test
    fun wide_showsThePageListBesideTheGeneralPageWithoutAClose() {
        launch(wide = true)

        rule.onNodeWithTag(SettingsTags.DESKTOP_NAV).assertIsDisplayed()
        // The right pane of the root is the General page, not the phone's grouped home.
        rule.onNodeWithTag(UiTags.SETTINGS_SEARCH).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.settings_parity_mobile_capabilities)).assertIsDisplayed()
        rule.onNode(topBarButtons).assertDoesNotExist()
        assertEquals(0, depths.last())
    }

    @Test
    fun wide_aPageFromTheListReplacesThePageOnTheRightInsteadOfStacking() {
        launch(wide = true)
        val account = hasText(text(R.string.settings_title_account)) and hasAnyAncestor(hasTestTag(SettingsTags.DESKTOP_NAV))
        val appearance = hasText(text(R.string.settings_appearance)) and hasAnyAncestor(hasTestTag(SettingsTags.DESKTOP_NAV))

        rule.onNode(account).performScrollTo().performClick()
        rule.waitForIdle()
        assertEquals(1, depths.last())
        rule.onNode(appearance).performScrollTo().performClick()
        rule.waitForIdle()
        assertEquals("a second pick replaces the first; Back is still one step from the root", 1, depths.last())

        pressArrow()
        assertEquals(0, depths.last())
        rule.onNodeWithTag(SettingsTags.DESKTOP_NAV).assertIsDisplayed()
    }

    @Test
    fun wide_aJumpAndItsOriginWorkLikeOnAPhone() {
        launch(wide = true)
        jumpTo(SettingsRoutes.ACCOUNT, from = AppTab.Chats)
        rule.onNodeWithTag(SettingsTags.DESKTOP_NAV).assertIsDisplayed()
        assertEquals(1, depths.last())

        pressArrow()

        assertEquals(listOf(AppTab.Chats), leftTo)
        assertEquals(0, depths.last())
    }
}
