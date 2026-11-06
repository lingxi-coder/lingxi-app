package com.lingxi.code.teams

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.R
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.TabHost
import com.lingxi.code.theme.LingXiTheme
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import com.lingxi.code.testing.finishComposeFixture
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The Teams tab on a device (SPEC-mobile-team-bot-ui section 6): the conversation list and the composer are driven with
 * hand-made state, so no Firebase project or account is needed; the signed-out gate runs against the real default app and
 * is skipped when someone is signed in on the device. The pure rules behind the screens (order, groups, previews, unread,
 * windowing, the composer lock) run on the JVM in `TeamsListLogicTest`, `TeamsStreamLogicTest` and `TeamsComposeLogicTest`.
 *
 * The content is laid out at a fixed dp size under a small fixed density, so every row is composed whatever the device is.
 */
@RunWith(AndroidJUnit4::class)
class TeamsTabUiTest {
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private val events = mutableListOf<String>()
    private val uid = "u1"
    private val team = TeamSummary("t1", "Crew", 4, "owner", 1)
    private val members = listOf(TeamMember("u1", "Me", "owner", 1), TeamMember("u2", "Ann", "member", 1), TeamMember("u3", "Bob", "member", 1), TeamMember("u9", "Zed", "member", 1))
    private val helper = TeamBotCard("sharedbot_helper", "Helper", "", TeamMemberRef("u1", 1), 1, "published", true, true, 0, JSONObject().put("mode", "all"), JSONObject().put("team",100).put("member",100).put("bot",100), null)
    private val teamsState = TeamsState(uid = uid, teams = listOf(team), detail = TeamDetail(team, members, emptyList()), selectedId = team.id, loaded = true)

    private fun pointer(id: String, seq: Long, atMs: Long?, author: String = "u2", preview: String = "hi") = TeamLastMessage("m$id", seq, atMs, "user", author, null, preview)
    private fun conversation(id: String, kind: String, title: String, participants: List<String>, lastSeq: Long, readSeq: Long, atMs: Long?, author: String = "u2", preview: String = "hi") =
        TeamMessageConversation(id, kind, title, participants.map { if (it.startsWith("sharedbot_")) "shared_bot:$it" else it }, lastSeq, lastSeq, readSeq, participants.filter { it.startsWith("sharedbot_") }, pointer(id, lastSeq, atMs, author, preview))

    private val base = 1_793_606_400_000L
    private val standard = listOf(
        conversation("a", "channel", "alpha", emptyList(), 3, 3, base),
        conversation("b", "channel", "beta", emptyList(), 9, 0, base + 3_600_000),
        conversation("dm1", "dm", "", listOf("user:u1", "user:u2"), 2, 0, base - 3_600_000, preview = "see you"),
        conversation("grp", "group", "Ann, Bob, You", listOf("user:u1", "user:u2", "user:u3"), 5, 5, base + 7_200_000),
        conversation("bd1", "bot_dm", "Helper", listOf("user:u1", helper.sharedBotId), 1, 1, base - 7_200_000),
        conversation("bd2", "bot_dm", "Helper", listOf("user:u9", helper.sharedBotId), 1, 1, base + 10_800_000),
        conversation("bd3", "bot_dm", "Other", listOf("user:u1", "sharedbot_other"), 1, 1, base - 10_800_000),
    )

    private fun messaging(
        conversations: List<TeamMessageConversation> = standard, offline: Boolean = false, error: String = "", directoryReady: Boolean = true,
        readsReady: Boolean = true, creation: JSONObject? = null,
    ) = TeamMessagingState(
        cache = TeamMessageCache(uid, team.id, 1, conversations = conversations, creation = creation), offline = offline, error = error, bots = listOf(helper),
        directoryReady = directoryReady, readsReady = readsReady, botsLoaded = true,
    )

    private fun showList(state: TeamMessagingState = messaging(), collapsed: Set<String> = emptySet(), unreadByTeam: Map<String, Int> = emptyMap(), open: String = "") {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalDensity provides Density(density = 0.5f, fontScale = 1f)) {
                    Box(Modifier.requiredSize(400.dp, 1600.dp)) {
                        TeamsListScreen(
                            uid = uid, teams = teamsState, messaging = state, collapsed = collapsed, unreadByTeam = unreadByTeam, openConversationId = open,
                            listState = rememberLazyListState(), onOpenConversation = { events += "open:$it" }, onSwitchTeam = { events += "switch" },
                            onNew = { events += "new:$it" }, onToggleSection = { events += "toggle:$it" }, onMarkRead = { events += "read:${it.id}" },
                            onReconnect = { events += "reconnect" }, onDiscardCreation = { events += "discard" },
                            onMessageBot = { events += "assistant:$it" },
                        )
                    }
                }
            }
        }
        rule.waitForIdle()
    }

    private fun top(tag: String) = rule.onNodeWithTag(tag).fetchSemanticsNode().boundsInRoot.top
    private fun text(id: Int) = rule.activity.getString(id)

    // --- The list --------------------------------------------------------------------------------------------------

    @Test fun conversationsAreGroupedIntoChannelsPeopleAndBotChatsEachNewestFirst() {
        showList()
        val order = listOf(
            TeamsTags.section(TeamsWorkspace.SECTION_CHANNELS), TeamsTags.row("b"), TeamsTags.row("a"),
            TeamsTags.section(TeamsWorkspace.SECTION_DIRECT), TeamsTags.row("grp"), TeamsTags.row("dm1"),
            // The viewer's own Bot chats first (newest first), then the Bot chats of other people.
            TeamsTags.section(TeamsWorkspace.SECTION_BOTS), TeamsTags.row("bd1"), TeamsTags.row("bd3"), TeamsTags.row("bd2"),
        )
        order.forEach { rule.onNodeWithTag(it).assertIsDisplayed() }
        val tops = order.map(::top)
        assertEquals("rows are not in the expected order: $tops", tops.sorted(), tops)
    }

    @Test fun unreadCountsShowOnTheirRowsAndStopAtNinetyNinePlus() {
        showList(messaging(standard + conversation("busy", "channel", "busy", emptyList(), 250, 0, base + 20_000_000)))
        rule.onNodeWithTag(TeamsTags.row("b")).assert(hasText("9"))
        rule.onNodeWithTag(TeamsTags.row("busy")).assert(hasText("99+"))
        rule.onNodeWithTag(TeamsTags.row("a")).assert(hasText("9").not())
    }

    @Test fun nothingIsMarkedUnreadUntilEveryReadMarkerHasArrived() {
        showList(messaging(readsReady = false))
        rule.onNodeWithTag(TeamsTags.row("b")).assert(hasText("9").not())
    }

    @Test fun aRowOpensItsConversation() {
        showList()
        rule.onNodeWithTag(TeamsTags.row("grp")).performClick()
        assertEquals(listOf("open:grp"), events)
    }

    @Test fun aSectionHeaderCollapsesItsRowsAndKeepsTheirUnreadCount() {
        showList(collapsed = setOf(TeamsWorkspace.SECTION_CHANNELS))
        rule.onNodeWithTag(TeamsTags.row("b")).assertDoesNotExist()
        rule.onNodeWithTag(TeamsTags.row("grp")).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.section(TeamsWorkspace.SECTION_CHANNELS)).assert(hasText("9"))
        rule.onNodeWithTag(TeamsTags.section(TeamsWorkspace.SECTION_CHANNELS)).performClick()
        assertEquals(listOf("toggle:${TeamsWorkspace.SECTION_CHANNELS}"), events)
    }

    @Test fun searchFiltersByTheRowLabelAndSaysWhenNothingMatches() {
        showList()
        rule.onNodeWithTag(TeamsTags.SEARCH).performClick()
        rule.onNodeWithTag(TeamsTags.SEARCH_FIELD).performTextInput("ALP")
        rule.waitForIdle()
        rule.onNodeWithTag(TeamsTags.row("a")).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.row("b")).assertDoesNotExist()
        rule.onNodeWithTag(TeamsTags.SEARCH_FIELD).performTextInput("zzz")
        rule.waitForIdle()
        rule.onNodeWithTag(TeamsTags.NO_MATCHES).assertIsDisplayed()
    }

    @Test fun anEmptyTeamOffersToStartAConversation() {
        showList(messaging(conversations = emptyList()).copy(bots = emptyList()))
        rule.onNodeWithTag(TeamsTags.NO_CONVERSATIONS).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.teams_conversation_new)).performClick()
        assertEquals(listOf("new:newMessage"), events)
    }

    @Test fun aMemberWithoutModelConfigurationStartsWithAvailableAssistants() {
        showList(messaging(conversations = emptyList()))
        rule.onNodeWithTag("teams.assistant.${helper.sharedBotId}").assertIsDisplayed().performClick()
        assertEquals(listOf("assistant:${helper.sharedBotId}"), events)
    }

    @Test fun theNewMenuOffersMessageChannelAndBot() {
        showList()
        rule.onNodeWithTag(TeamsTags.NEW).performClick()
        rule.onNodeWithText(text(R.string.teams_new_channel_title)).performClick()
        rule.onNodeWithTag(TeamsTags.NEW).performClick()
        rule.onNodeWithText(text(R.string.teams_new_bot_title)).performClick()
        rule.onNodeWithTag(TeamsTags.NEW).performClick()
        rule.onNodeWithText(text(R.string.teams_new_message_title)).performClick()
        assertEquals(listOf("new:newChannel", "new:newBot", "new:newMessage"), events)
    }

    @Test fun theTeamChipOpensTheSwitcher() {
        showList(unreadByTeam = mapOf("t2" to 3))
        rule.onNodeWithTag(TeamsTags.TEAM_CHIP).performClick()
        assertEquals(listOf("switch"), events)
    }

    @Test fun offlineShowsASavedMessagesBannerWithReconnect() {
        showList(messaging(offline = true))
        rule.onNodeWithTag(TeamsTags.OFFLINE).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.row("b")).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.mcp_reconnect)).performClick()
        assertEquals(listOf("reconnect"), events)
    }

    @Test fun noOfflineBannerFlashesBeforeTheDirectoryHasLoaded() {
        showList(messaging(offline = true, directoryReady = false))
        rule.onNodeWithTag(TeamsTags.OFFLINE).assertDoesNotExist()
        rule.onNodeWithTag(TeamsTags.LOADING).assertIsDisplayed()
    }

    @Test fun invalidServerDataStopsTheListInsteadOfShowingAPartialOne() {
        showList(messaging(error = "INVALID_DATA"))
        rule.onNodeWithTag(TeamsTags.INVALID_DATA).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.row("b")).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("reconnect"), events)
    }

    @Test fun longPressMarksAnUnreadConversationReadOnlyWhileOnline() {
        showList()
        rule.onNodeWithTag(TeamsTags.row("b")).performTouchInput { longClick() }
        rule.onNodeWithText(text(R.string.teams_mark_read)).performClick()
        assertEquals(listOf("read:b"), events)
    }

    @Test fun offlineAndReadConversationsOfferNoMarkRead() {
        showList(messaging(offline = true))
        rule.onNodeWithTag(TeamsTags.row("b")).performTouchInput { longClick() }
        rule.onNodeWithText(text(R.string.teams_mark_read)).assertDoesNotExist()
    }

    @Test fun aConversationBeingCreatedStaysVisibleWithoutShowingTheEmptyState() {
        showList(messaging(conversations = emptyList(), creation = JSONObject().put("title", "Launch plan").put("attempted", false)))
        rule.onNodeWithText("Launch plan").assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.NO_CONVERSATIONS).assertDoesNotExist()
    }

    // --- The composer ----------------------------------------------------------------------------------------------

    private var draft by mutableStateOf(ComposerDraft())

    private fun showComposer(conversation: TeamMessageConversation, state: TeamMessagingState = messaging(), initial: ComposerDraft = ComposerDraft()) {
        draft = initial
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalDensity provides Density(density = 0.5f, fontScale = 1f)) {
                    Box(Modifier.requiredSize(400.dp, 1000.dp)) {
                        TeamComposer(
                            conversation = conversation, label = "Ann", uid = uid, messaging = state, draft = draft, replyTo = null, editing = null, replyAuthor = "",
                            onDraft = { draft = it }, onCancelReply = { events += "cancelReply" }, onCancelEdit = { events += "cancelEdit" },
                            onSend = { sent, targets, done -> events += "send:$sent:$targets"; done() },
                        )
                    }
                }
            }
        }
        rule.waitForIdle()
    }

    @Test fun someoneElsesBotChatReplacesTheComposerWithAnExplanation() {
        showComposer(standard.first { it.id == "bd2" })
        rule.onNodeWithTag(TeamsTags.COMPOSER_LOCK).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.COMPOSER_FIELD).assertDoesNotExist()
    }

    @Test fun aBotChatThatTheBotsAvailabilityHasNotConfirmedYetIsLocked() {
        showComposer(standard.first { it.id == "bd1" }, messaging().copy(bots = emptyList(), botsLoaded = false))
        rule.onNodeWithTag(TeamsTags.COMPOSER_LOCK).assertIsDisplayed()
    }

    @Test fun sendNeedsTextAndHandsTheTypedTextOverOnce() {
        showComposer(standard.first { it.id == "dm1" })
        rule.onNodeWithTag(TeamsTags.SEND).assertIsNotEnabled()
        rule.onNodeWithTag(TeamsTags.COMPOSER_FIELD).performTextInput("hello there")
        rule.onNodeWithTag(TeamsTags.SEND).assertIsEnabled()
        rule.onNodeWithTag(TeamsTags.SEND).performClick()
        rule.waitForIdle()
        assertEquals(listOf("send:hello there:[]"), events)
        rule.onNodeWithTag(TeamsTags.SEND).assertIsNotEnabled()
    }

    @Test fun anOversizedMessageShowsTheHintAndCannotBeSent() {
        showComposer(standard.first { it.id == "dm1" }, initial = ComposerDraft(text = "a".repeat(TeamsComposeLogic.MESSAGE_BYTES + 1)))
        rule.onNodeWithText(text(R.string.teams_composer_too_long)).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.SEND).assertIsNotEnabled()
    }

    @Test fun askingABotPicksItFromTheListAndKeepsItAsAChip() {
        showComposer(conversation("g", "group", "Crew chat", listOf("user:u1", "user:u2", helper.sharedBotId), 1, 1, base))
        rule.onNodeWithTag(TeamsTags.ASK_BOT).performClick()
        rule.waitForIdle()
        rule.onNodeWithText("Helper").performClick()
        rule.waitForIdle()
        assertEquals(setOf(helper.sharedBotId), draft.targets)
        rule.onNode(hasContentDescription(rule.activity.getString(R.string.teams_composer_remove_bot_fmt, "Helper"))).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.COMPOSER_FIELD).performTextInput("please summarise")
        rule.onNodeWithTag(TeamsTags.SEND).performClick()
        rule.waitForIdle()
        assertEquals(listOf("send:please summarise:[${helper.sharedBotId}]"), events)
    }

    @Test fun aBotThatLeftTheChatBlocksSendingUntilTheChipIsRemoved() {
        showComposer(
            conversation("g", "group", "Crew chat", listOf("user:u1", "user:u2", helper.sharedBotId), 1, 1, base),
            initial = ComposerDraft(text = "hello", targets = setOf("sharedbot_gone")),
        )
        rule.onNodeWithText(text(R.string.teams_error_bot_access_changed)).assertIsDisplayed()
        rule.onNodeWithTag(TeamsTags.SEND).assertIsNotEnabled()
    }

    // --- The tab against the real default app ------------------------------------------------------------------------

    private class RecordingHost : TabHost {
        val calls = mutableListOf<String>()
        override fun select(tab: AppTab) { calls += "select:${tab.key}" }
        override fun openSettings(route: String, origin: AppTab?) { calls += "settings:$route:${origin?.key}" }
        override fun reportDepth(tab: AppTab, depth: Int) { calls += "depth:${tab.key}:$depth" }
    }

    private fun showTab(host: RecordingHost, joinCode: String?) {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalTabHost provides host) {
                    TeamsTab(joinCode = joinCode, onJoinCodeHandled = { events += "joinHandled" })
                }
            }
        }
        rule.waitForIdle()
    }

    @Test fun signedOutTheTabShowsTheGateAndSignInJumpsToAccountSettingsFromTeams() {
        assumeTrue("A user is signed in on this device", FirebaseAuth.getInstance().currentUser == null)
        val host = RecordingHost()
        showTab(host, null)
        rule.onNodeWithTag(TeamsTags.SIGN_IN).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.remote_sign_in)).performClick()
        assertTrue(host.calls.toString(), "settings:${SettingsRoutes.ACCOUNT}:teams" in host.calls)
    }

    @Test fun aJoinCodeWaitsForSignInInsteadOfBeingConsumed() {
        assumeTrue("A user is signed in on this device", FirebaseAuth.getInstance().currentUser == null)
        showTab(RecordingHost(), "ABCD-EFGH")
        rule.onNodeWithTag(TeamsTags.SIGN_IN).assertIsDisplayed()
        assertTrue("the code was handled while signed out: $events", "joinHandled" !in events)
    }
}
