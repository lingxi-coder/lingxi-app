package com.lingxi.code.tabs

import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The app shell against stand-ins for its tab bodies (SPEC-mobile-team-bot-ui sections 2 and 9.2): the
 * bar's visibility, selection and re-tap, per-tab state, the Back chain, the wide rail and the badges.
 *
 * Each stand-in keeps the promises a real tab body makes to the shell: it reports its pushed depth,
 * pops one screen on Back, and answers a re-tap of its own tab. The pure rules behind the bar are
 * covered by `TabLogicTest` on the JVM; this class checks that the shell applies them.
 *
 * The content is laid out at a fixed dp size under a small fixed density, so the phone / wide choice
 * does not depend on the device that runs the test.
 */
@RunWith(AndroidJUnit4::class)
class TabShellUiTest {
    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private lateinit var state: TabShellState
    private val log = mutableListOf<String>()
    private val botsBadge = MutableStateFlow(0)
    private val teamsBadge = MutableStateFlow(0)
    private var widthDp by mutableStateOf(PHONE_WIDTH)
    private var overlayOpen by mutableStateOf(false)

    /**
     * [edgeToEdge] lets the content see the real system-bar and keyboard insets, as `MainActivity` does
     * (`enableEdgeToEdge()`); the plain test activity otherwise consumes them in its decor view.
     */
    private fun launch(width: Dp = PHONE_WIDTH, edgeToEdge: Boolean = false) {
        widthDp = width
        if (edgeToEdge) rule.runOnUiThread { WindowCompat.setDecorFitsSystemWindows(rule.activity.window, false) }
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalDensity provides Density(density = 0.5f, fontScale = 1f)) {
                    val shell = rememberTabShellState()
                    state = shell
                    Box(Modifier.requiredSize(widthDp, 800.dp)) {
                        MainTabShell(
                            state = shell,
                            overlayOpen = overlayOpen,
                            chats = { StandInChats(shell, log) },
                            settings = { StandInTab(AppTab.Settings, it, log) },
                            bots = { StandInTab(AppTab.Bots, it, log) },
                            teams = { StandInTab(AppTab.Teams, it, log) },
                            botsBadge = botsBadge,
                            teamsBadge = teamsBadge,
                        )
                    }
                }
            }
        }
        rule.waitForIdle()
    }

    private fun tap(tab: AppTab) {
        rule.onNodeWithTag(UiTags.tab(tab.key)).performClick()
        rule.waitForIdle()
    }

    private fun pressBack() {
        rule.runOnUiThread { rule.activity.onBackPressedDispatcher.onBackPressed() }
        rule.waitForIdle()
    }

    private fun bounds(tag: String) = rule.onNodeWithTag(tag).fetchSemanticsNode().boundsInRoot

    private fun navigationBarInsetPx(): Int = rule.runOnUiThread {
        ViewCompat.getRootWindowInsets(rule.activity.window.decorView)
            ?.getInsets(WindowInsetsCompat.Type.navigationBars())?.bottom ?: 0
    }

    private fun imeVisible(): Boolean = rule.runOnUiThread {
        ViewCompat.getRootWindowInsets(rule.activity.window.decorView)?.isVisible(WindowInsetsCompat.Type.ime()) == true
    }

    private fun setIme(visible: Boolean) = rule.runOnUiThread {
        val controller = WindowCompat.getInsetsController(rule.activity.window, rule.activity.window.decorView)
        if (visible) controller.show(WindowInsetsCompat.Type.ime()) else controller.hide(WindowInsetsCompat.Type.ime())
    }

    private fun assertBarShown() {
        rule.onNodeWithTag(UiTags.TABS_BAR).assertIsDisplayed()
        rule.onNodeWithTag(UiTags.TABS_RAIL).assertDoesNotExist()
    }

    private fun assertNoBar() {
        rule.onNodeWithTag(UiTags.TABS_BAR).assertDoesNotExist()
    }

    private fun assertSelected(tab: AppTab) {
        assertEquals(tab, rule.runOnUiThread { state.selected })
        rule.onNodeWithTag(UiTags.tab(tab.key)).assertIsSelected()
    }

    // --- the bar ----------------------------------------------------------------------------

    @Test
    fun phone_showsTheBarWithFourTabsAndSwitchesBetweenThem() {
        launch()

        assertBarShown()
        AppTab.entries.forEach { rule.onNodeWithTag(UiTags.tab(it.key)).assertIsDisplayed() }
        assertSelected(AppTab.Chats)

        for (tab in listOf(AppTab.Bots, AppTab.Teams, AppTab.Settings, AppTab.Chats)) {
            tap(tab)
            assertSelected(tab)
            assertBarShown()
        }
    }

    @Test
    fun phone_hidesTheBarOnAPushedScreenAndBackBringsItBack() {
        launch()
        tap(AppTab.Bots)

        rule.onNodeWithTag("bots.push").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("bots.depth").assertTextEquals("depth=1")
        assertNoBar()

        pressBack()
        rule.onNodeWithTag("bots.depth").assertTextEquals("depth=0")
        assertBarShown()
        assertSelected(AppTab.Bots)
    }

    @Test
    fun phone_hidesTheBarWhileTheChatsDrawerIsOpenButOnlyOnChats() {
        launch()

        rule.onNodeWithTag("chats.drawer").performClick()
        rule.waitForIdle()
        assertNoBar()
        rule.onNodeWithTag("chats.drawer").performClick()
        rule.waitForIdle()
        assertBarShown()

        // A drawer left open behind another tab says nothing about that tab.
        rule.runOnUiThread { state.reportChatsChrome(pushedPages = 0, drawerOpen = true, fullScreenOverlay = false) }
        rule.runOnUiThread { state.select(AppTab.Bots) }
        rule.waitForIdle()
        assertBarShown()
    }

    @Test
    fun fullScreenOverlaysHideTheBar() {
        launch()

        // One drawn above the shell (onboarding, the terminal, the import preview).
        rule.runOnUiThread { overlayOpen = true }
        rule.waitForIdle()
        assertNoBar()
        rule.runOnUiThread { overlayOpen = false }
        rule.waitForIdle()
        assertBarShown()

        // One owned by Chats (a running app, the voice flow) covers the bar while Chats is showing.
        rule.onNodeWithTag("chats.overlay").performClick()
        rule.waitForIdle()
        assertNoBar()
        rule.runOnUiThread { state.select(AppTab.Teams) }
        rule.waitForIdle()
        assertBarShown()
    }

    @Test
    fun badgesCountWaitingBotsAndUnreadTeamsCappedAt99() {
        launch()
        val bots = rule.activity.getString(R.string.remote_bot_waiting_fmt, 3)
        val teams = rule.activity.getString(R.string.tabs_badge_unread_fmt, 120)

        rule.runOnUiThread { botsBadge.value = 3; teamsBadge.value = 120 }
        rule.waitForIdle()
        rule.onNodeWithTag(UiTags.tab("bots")).assert(hasContentDescription(bots, substring = true))
        rule.onNodeWithTag(UiTags.tab("teams")).assert(hasContentDescription(teams, substring = true))

        rule.runOnUiThread { botsBadge.value = 0; teamsBadge.value = 0 }
        rule.waitForIdle()
        rule.onNodeWithTag(UiTags.tab("bots")).assert(!hasContentDescription(bots, substring = true))
        rule.onNodeWithTag(UiTags.tab("teams")).assert(!hasContentDescription(teams, substring = true))
    }

    @Test
    fun phone_hidesTheBarWhileTheKeyboardIsUp() {
        launch(edgeToEdge = true)
        assertBarShown()

        rule.onNodeWithTag("chats.input").performClick()
        setIme(visible = true)
        val shown = runCatching { rule.waitUntil(5_000) { imeVisible() } }.isSuccess
        assumeTrue("needs a soft keyboard on this device", shown)
        rule.waitForIdle()
        assertNoBar()

        setIme(visible = false)
        rule.waitUntil(5_000) { !imeVisible() }
        rule.waitForIdle()
        assertBarShown()
    }

    @Test
    fun phone_theBarPadsTheNavigationBarInsetSoTheContentDoesNotPadItAgain() {
        launch(edgeToEdge = true)
        val inset = navigationBarInsetPx()
        assumeTrue("needs a navigation-bar inset to tell the two cases apart", inset > 0)

        // Bar shown: the probe sits flush on the bottom edge of the content, and that edge is the bar's top.
        assertEquals(bounds("chats.root").bottom, bounds("chats.probe").bottom, 1f)
        assertEquals(bounds("chats.root").bottom, bounds(UiTags.TABS_BAR).top, 1f)

        // Bar hidden (a pushed page): the content pads the inset itself again.
        rule.onNodeWithTag("chats.push").performClick()
        rule.waitForIdle()
        assertNoBar()
        assertEquals(bounds("chats.root").bottom - inset, bounds("chats.probe").bottom, 1f)
    }

    // --- tab state --------------------------------------------------------------------------

    @Test
    fun chatsStaysComposedWhileAnotherTabIsSelected() {
        launch()
        rule.onNodeWithTag("chats.draft").performClick()
        rule.onNodeWithTag("chats.draft").performClick()
        rule.onNodeWithTag("chats.drafts").assertTextEquals("drafts=2")

        tap(AppTab.Settings)
        // Out of sight and out of the accessibility tree ...
        rule.onNodeWithTag("chats.drafts").assertDoesNotExist()
        rule.onNodeWithTag("body.settings").assertIsDisplayed()
        tap(AppTab.Chats)

        // ... but never recreated: this counter is plain `remember`, not saved state.
        rule.onNodeWithTag("chats.drafts").assertTextEquals("drafts=2")
    }

    @Test
    fun everyTabKeepsItsOwnStack() {
        launch()
        tap(AppTab.Settings)
        rule.onNodeWithTag("settings.push").performClick()
        rule.onNodeWithTag("settings.push").performClick()
        rule.onNodeWithTag("settings.depth").assertTextEquals("depth=2")

        // A link elsewhere (the bar is hidden on a pushed screen) and back through the bar.
        rule.runOnUiThread { state.select(AppTab.Teams) }
        rule.waitForIdle()
        assertBarShown()
        tap(AppTab.Settings)

        rule.onNodeWithTag("settings.depth").assertTextEquals("depth=2")
        assertNoBar()
    }

    @Test
    fun retapOnATabRootScrollsItToTheTop() {
        launch()
        tap(AppTab.Bots)
        tap(AppTab.Bots)
        tap(AppTab.Bots)

        assertEquals(listOf("bots.top", "bots.top"), log.filter { it.startsWith("bots.") })
        assertSelected(AppTab.Bots)
    }

    @Test
    fun selectingAnotherTabDoesNotCountAsARetap() {
        launch()
        tap(AppTab.Bots)
        tap(AppTab.Teams)

        assertTrue(log.isEmpty())
    }

    // --- Back -------------------------------------------------------------------------------

    @Test
    fun back_onAnotherTabRootReturnsToChats() {
        launch()
        tap(AppTab.Settings)

        pressBack()

        assertSelected(AppTab.Chats)
        assertBarShown()
    }

    @Test
    fun back_popsTheTabStackBeforeLeavingTheTab() {
        launch()
        tap(AppTab.Teams)
        rule.onNodeWithTag("teams.push").performClick()

        pressBack()
        rule.onNodeWithTag("teams.depth").assertTextEquals("depth=0")
        assertSelected(AppTab.Teams)
        assertEquals(listOf("teams.pop"), log)

        pressBack()
        assertSelected(AppTab.Chats)
    }

    @Test
    fun back_neverReachesAChatsPageTheUserCannotSee() {
        launch()
        rule.onNodeWithTag("chats.push").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("chats.pages").assertTextEquals("pages=1")
        assertNoBar()

        // A link selects Bots while a Chats page is open.
        rule.runOnUiThread { state.select(AppTab.Bots) }
        rule.waitForIdle()
        assertBarShown()
        pressBack()

        // Back left Bots for Chats; the Chats page stayed open and nothing in Chats handled the press.
        assertEquals(AppTab.Chats, rule.runOnUiThread { state.selected })
        assertTrue("Chats must not handle Back while hidden: $log", log.none { it == "chats.closePage" })
        rule.onNodeWithTag("chats.pages").assertTextEquals("pages=1")

        // Now that Chats is showing again, its own page is the next thing Back closes.
        pressBack()
        assertEquals(listOf("chats.closePage"), log)
        rule.onNodeWithTag("chats.pages").assertTextEquals("pages=0")
        assertBarShown()
    }

    // --- links and shortcuts ----------------------------------------------------------------

    @Test
    fun openSettingsSelectsSettingsAndPublishesTheJumpOnce() {
        launch()

        rule.runOnUiThread { state.openSettings(SettingsRoutes.ACCOUNT, AppTab.Chats) }
        rule.waitForIdle()

        assertSelected(AppTab.Settings)
        val jump = rule.runOnUiThread { state.settingsJump }
        assertNotNull(jump)
        assertEquals(SettingsRoutes.ACCOUNT, jump!!.route)
        assertEquals(AppTab.Chats, jump.origin)

        rule.runOnUiThread { state.onSettingsJumpHandled(jump.id) }
        assertNull(rule.runOnUiThread { state.settingsJump })
    }

    @Test
    fun requestsForBotsAndTeamsSelectTheirTabs() {
        launch()

        rule.runOnUiThread { state.requestBots("host-1", "session-9") }
        rule.waitForIdle()
        assertSelected(AppTab.Bots)
        val request = rule.runOnUiThread { state.botsRequest }
        assertEquals("host-1", request?.hostId)
        assertEquals("session-9", request?.sessionId)

        rule.runOnUiThread { state.requestTeams("CODE42") }
        rule.waitForIdle()
        assertSelected(AppTab.Teams)
        assertEquals("CODE42", rule.runOnUiThread { state.teamsJoinCode })
    }

    // --- the wide layout --------------------------------------------------------------------

    @Test
    fun wide_showsTheRailInsteadOfTheBar() {
        launch(width = WIDE_WIDTH)

        rule.onNodeWithTag(UiTags.TABS_RAIL).assertIsDisplayed()
        rule.onNodeWithTag(UiTags.TABS_BAR).assertDoesNotExist()
        tap(AppTab.Bots)
        assertSelected(AppTab.Bots)
        rule.onNodeWithTag("bots.wide").assertTextEquals("wide=true")
    }

    @Test
    fun wide_keepsTheRailOnPushedScreensAndHidesItUnderOverlays() {
        launch(width = WIDE_WIDTH)
        tap(AppTab.Bots)

        rule.onNodeWithTag("bots.push").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("bots.depth").assertTextEquals("depth=1")
        rule.onNodeWithTag(UiTags.TABS_RAIL).assertIsDisplayed()

        rule.runOnUiThread { overlayOpen = true }
        rule.waitForIdle()
        rule.onNodeWithTag(UiTags.TABS_RAIL).assertDoesNotExist()
    }

    @Test
    fun wide_retapOnATabWithPushedScreensPopsToItsRoot() {
        launch(width = WIDE_WIDTH)
        tap(AppTab.Bots)
        rule.onNodeWithTag("bots.push").performClick()
        rule.waitForIdle()

        tap(AppTab.Bots)

        rule.onNodeWithTag("bots.depth").assertTextEquals("depth=0")
        assertEquals(listOf("bots.toRoot"), log)
    }

    @Test
    fun wide_retapOnChatsClosesItsPushedPages() {
        launch(width = WIDE_WIDTH)
        rule.onNodeWithTag("chats.push").performClick()
        rule.waitForIdle()
        rule.onNodeWithTag("chats.pages").assertTextEquals("pages=1")
        rule.onNodeWithTag(UiTags.TABS_RAIL).assertIsDisplayed()

        tap(AppTab.Chats)

        rule.onNodeWithTag("chats.pages").assertTextEquals("pages=0")
    }

    @Test
    fun breakpointIs840dp() {
        // The density is 0.5, so only even dp values are whole pixels: 838 is the last width below 840.
        launch(width = 838.dp)
        assertBarShown()
        rule.onNodeWithTag("chats.wide").assertTextEquals("wide=false")

        rule.runOnUiThread { widthDp = 840.dp }
        rule.waitForIdle()
        rule.onNodeWithTag(UiTags.TABS_RAIL).assertIsDisplayed()
        rule.onNodeWithTag(UiTags.TABS_BAR).assertDoesNotExist()
        rule.onNodeWithTag("chats.wide").assertTextEquals("wide=true")
    }

    @Test
    fun crossingTheBreakpointKeepsChatsAndTheSelectedTab() {
        launch()
        rule.onNodeWithTag("chats.draft").performClick()
        tap(AppTab.Settings)

        rule.runOnUiThread { widthDp = WIDE_WIDTH }
        rule.waitForIdle()
        rule.onNodeWithTag(UiTags.TABS_RAIL).assertIsDisplayed()
        assertSelected(AppTab.Settings)

        rule.runOnUiThread { widthDp = PHONE_WIDTH }
        rule.waitForIdle()
        assertBarShown()
        assertSelected(AppTab.Settings)

        tap(AppTab.Chats)
        rule.onNodeWithTag("chats.drafts").assertTextEquals("drafts=1")
    }

    private companion object {
        val PHONE_WIDTH = 400.dp
        val WIDE_WIDTH = 900.dp
    }
}

/**
 * What the real Chats tab (`RootScreen`) tells the shell: its pushed pages, whether the drawer is open
 * and whether a full-screen overlay covers it. The counters use plain `remember` on purpose, so a
 * recreated Chats shows up as a reset.
 */
@Composable
private fun StandInChats(state: TabShellState, log: MutableList<String>) {
    var drafts by remember { mutableIntStateOf(0) }
    var pages by remember { mutableIntStateOf(0) }
    var drawerOpen by remember { mutableStateOf(false) }
    var overlay by remember { mutableStateOf(false) }
    LaunchedEffect(pages, drawerOpen, overlay) { state.reportChatsChrome(pages, drawerOpen, overlay) }
    BackHandler(enabled = pages > 0) {
        log += "chats.closePage"
        pages -= 1
    }
    OnTabReselected(AppTab.Chats) { depth -> if (depth > 0) pages = 0 }
    var input by remember { mutableStateOf("") }
    Box(Modifier.fillMaxSize().testTag("chats.root")) {
        Column(Modifier.fillMaxSize()) {
            Text("drafts=$drafts", Modifier.testTag("chats.drafts"))
            Text("pages=$pages", Modifier.testTag("chats.pages"))
            Text("wide=${LocalWideLayout.current}", Modifier.testTag("chats.wide"))
            Button(onClick = { drafts += 1 }, Modifier.testTag("chats.draft")) { Text("draft") }
            Button(onClick = { pages += 1 }, Modifier.testTag("chats.push")) { Text("push page") }
            Button(onClick = { drawerOpen = !drawerOpen }, Modifier.testTag("chats.drawer")) { Text("drawer") }
            Button(onClick = { overlay = !overlay }, Modifier.testTag("chats.overlay")) { Text("overlay") }
            BasicTextField(value = input, onValueChange = { input = it }, modifier = Modifier.testTag("chats.input"))
        }
        // On the bottom edge, padding the navigation-bar inset the way the real composer area does.
        Text(
            "probe",
            Modifier.align(Alignment.BottomStart).windowInsetsPadding(WindowInsets.navigationBars).testTag("chats.probe"),
        )
    }
}

/**
 * A Bots / Teams / Settings body in miniature: it reports its depth, pops one screen on Back and
 * answers a re-tap of its tab, the three things section 2.3 asks of every tab.
 */
@Composable
private fun StandInTab(tab: AppTab, modifier: Modifier, log: MutableList<String>) {
    val host = LocalTabHost.current
    var depth by rememberSaveable { mutableIntStateOf(0) }
    LaunchedEffect(depth) { host.reportDepth(tab, depth) }
    BackHandler(enabled = depth > 0) {
        log += "${tab.key}.pop"
        depth -= 1
    }
    OnTabReselected(tab) { pushed ->
        if (pushed > 0) {
            log += "${tab.key}.toRoot"
            depth = 0
        } else {
            log += "${tab.key}.top"
        }
    }
    Column(modifier.testTag("body.${tab.key}")) {
        Text("depth=$depth", Modifier.testTag("${tab.key}.depth"))
        Text("wide=${LocalWideLayout.current}", Modifier.testTag("${tab.key}.wide"))
        Button(onClick = { depth += 1 }, Modifier.testTag("${tab.key}.push")) { Text("push") }
    }
}
