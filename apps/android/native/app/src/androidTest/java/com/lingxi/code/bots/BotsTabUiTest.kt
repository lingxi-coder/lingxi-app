package com.lingxi.code.bots

import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.getBoundsInRoot
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotThreadsStateFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteFailureFfi
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.remote.RemoteBotChatsState
import com.lingxi.code.remote.RemoteBotsState
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * What the Bots tab draws for each state of SPEC-mobile-team-bot-ui.md section 4.5 and what its rows ask for
 * (section 4.3). The rows are made by `buildTree`, whose rules the JVM tests cover; these show the drawing of them.
 */
@RunWith(AndroidJUnit4::class)
class BotsTabUiTest {
    // These view assertions use the main dispatcher; no separate coroutine test scheduler is needed.
    @get:Rule val compose = createComposeRule()

    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private fun text(id: Int, vararg args: Any) = context.getString(id, *args)

    /** What the rows asked for, in order. */
    private val events = mutableListOf<String>()

    private val actions = BotsActions(
        onSignIn = { events += "signIn" },
        onRetryHosts = { events += "retryHosts" },
        onToggleHost = { hostId, collapsed -> events += "toggleHost:$hostId:$collapsed" },
        onOpenHost = { events += "openHost:$it" },
        onToggleBot = { events += "toggleBot:${it.facts.hostId}/${it.bot.botId}" },
        onOpenChat = { events += "openChat:${it.chat.sessionId}" },
        onNewChat = { events += "newChat:${it.botId}" },
        onShowMore = { hostId, botId -> events += "more:$hostId/$botId" },
        onShowLess = { hostId, botId -> events += "less:$hostId/$botId" },
        onConnect = { events += "connect:$it" },
        onRetryChats = { hostId, botId -> events += "retryChats:$hostId/$botId" },
        onRetryConnection = { events += "retryConnection" },
        onRepair = { events += "repair:$it" },
    )

    // --- builders -------------------------------------------------------------------------------------------------

    private fun host(id: String, online: Boolean = true, paired: Boolean = true, name: String = "Host $id", lastSeen: Long = 0L) =
        RemoteHostFfi(id, name, "macos", "1.0", online, lastSeen.toULong(), paired)

    private fun liveBot(id: String, name: String = "Bot $id", description: String = "does $id", status: RemoteBotStatusFfi = RemoteBotStatusFfi.IDLE, waiting: Int = 0) =
        RemoteBotSummaryFfi(id, name, description, status, waiting.toUInt(), null)

    private fun liveBots(hostId: String, vararg bots: RemoteBotSummaryFfi) = RemoteBotsState(hostId, RemoteBotsStateFfi.Bots(bots.toList()))

    private fun liveChat(id: String, title: String = "Chat $id", status: RemoteSessionStatusFfi = RemoteSessionStatusFfi.IDLE, pending: RemotePendingActionFfi? = null) =
        RemoteSessionSummaryFfi(id, title, "", status, pending, null, 0u)

    private fun heldChats(hostId: String, botId: String, threads: List<RemoteSessionSummaryFfi>?, error: RemoteSessionErrorFfi? = null) =
        RemoteBotChatsState(hostId, mapOf(botId to RemoteBotThreadsStateFfi(threads, error)))

    private fun connection(hostId: String, phase: RemoteConnectionPhaseFfi, code: String? = null, failure: RemoteFailureFfi? = null, everReady: Boolean = false) =
        RemoteConnectionStateFfi(hostId, phase, null, code, failure, 0u, null, null, everReady)

    private fun ready(hostId: String) = connection(hostId, RemoteConnectionPhaseFfi.READY, everReady = true)

    private fun savedBot(id: String, chats: List<ChatItem>? = null) = BotItem(id, "Bot $id", "does $id", chats = chats)

    private fun saved(hostId: String, vararg bots: BotItem, at: Long = System.currentTimeMillis() - 3 * HOUR - 5_000L) =
        BotsSnapshot("u1", hostId, at, bots.toList())

    private fun tree(
        hosts: List<RemoteHostFfi>,
        connection: RemoteConnectionStateFfi? = null,
        bots: RemoteBotsState = RemoteBotsState(),
        chats: RemoteBotChatsState = RemoteBotChatsState(),
        snapshots: List<BotsSnapshot> = emptyList(),
        collapsed: Set<String> = emptySet(),
        expanded: Set<String>? = emptySet(),
        showAll: Set<String> = emptySet(),
    ) = buildTree(TreeInput(hosts, connection, bots, chats, snapshots.associateBy { it.hostId }, collapsed, expanded, showAll))

    private fun showRoot(
        state: RootState,
        rows: List<TreeRow> = emptyList(),
        loading: Boolean = false,
        hostsError: Boolean = false,
        creating: List<String> = emptyList(),
        errors: Map<String, Int> = emptyMap(),
    ) = compose.setContent {
        LingXiTheme(darkTheme = false) {
            BotsRootContent(state, loading, hostsError, rows, creating, errors, actions, rememberLazyListState())
        }
    }

    private fun showTree(
        rows: List<TreeRow>,
        hostsError: Boolean = false,
        creating: List<String> = emptyList(),
        errors: Map<String, Int> = emptyMap(),
    ) = showRoot(RootState.Tree, rows, hostsError = hostsError, creating = creating, errors = errors)

    private fun below(upper: String, lower: String): Boolean =
        compose.onNodeWithText(upper).getBoundsInRoot().top < compose.onNodeWithText(lower).getBoundsInRoot().top

    // --- the whole-page states ------------------------------------------------------------------------------------

    @Test fun signedOutShowsTheGateAndLeadsToSignIn() {
        showRoot(RootState.SignedOut)
        compose.onNodeWithTag(BotsTags.SIGNED_OUT).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_signed_out_title)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_signed_out_body)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_sign_in)).performClick()
        assertEquals(listOf("signIn"), events)
    }

    @Test fun theTabTitlesItselfBots() {
        showRoot(RootState.NoComputers)
        compose.onNodeWithText(text(R.string.remote_bots)).assertIsDisplayed()
    }

    @Test fun listedComputersLoadingShowsASpinnerAndLoadingText() {
        showRoot(RootState.Loading)
        compose.onNodeWithTag(BotsTags.LOADING).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_loading)).assertIsDisplayed()
    }

    @Test fun theTreeWaitsForItsSavedChoicesTooWithoutShowingRows() {
        showRoot(RootState.Tree, rows = tree(listOf(host("h1"))), loading = true)
        compose.onNodeWithTag(BotsTags.LOADING).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.TREE).assertDoesNotExist()
    }

    @Test fun aHostsErrorShowsTheMessageAndRetries() {
        showRoot(RootState.Error)
        compose.onNodeWithTag(BotsTags.HOSTS_ERROR).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_hosts_error)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retryHosts"), events)
    }

    @Test fun noComputersExplainsHowToGetOne() {
        showRoot(RootState.NoComputers)
        compose.onNodeWithTag(BotsTags.NO_COMPUTERS).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_no_computers)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_no_computers_help)).assertIsDisplayed()
    }

    @Test fun aHostsErrorOverAListedTreeKeepsTheTreeAndOffersARetry() {
        showTree(tree(listOf(host("h1"))), hostsError = true)
        compose.onNodeWithTag(BotsTags.HOSTS_ERROR).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.host("h1")).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retryHosts"), events)
    }

    // --- computer headers -----------------------------------------------------------------------------------------

    @Test fun eachComputerHeaderSaysWhereTheComputerStands() {
        val twoDaysAgo = System.currentTimeMillis() - 2 * DAY - 5_000L
        showTree(
            tree(
                listOf(
                    host("h1", name = "Studio"),
                    host("h2", name = "Laptop"),
                    host("h3", online = false, name = "Old box", lastSeen = twoDaysAgo),
                    host("h4", paired = false, name = "New box"),
                ),
                ready("h1"), liveBots("h1"),
            ),
        )
        compose.onNodeWithText("Studio").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.mobile_bots_connected)).assertIsDisplayed()
        compose.onNodeWithText("Laptop").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_online)).assertIsDisplayed()
        compose.onNodeWithText(
            text(R.string.remote_offline) + " · " + text(R.string.mobile_bots_last_seen_fmt, text(R.string.session_relative_days_ago_fmt, 2)),
        ).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_online) + " · " + text(R.string.remote_not_paired)).assertIsDisplayed()
    }

    @Test fun theConnectedComputerComesFirst() {
        showTree(tree(listOf(host("h1", name = "Laptop"), host("h2", name = "Studio")), ready("h2"), liveBots("h2")))
        assertTrue(below("Studio", "Laptop"))
    }

    @Test fun tappingAHeaderCollapsesItAndTappingACollapsedOneOpensIt() {
        val rows = tree(listOf(host("h1"), host("h2")), collapsed = setOf("h2"))
        showTree(rows)
        compose.onNodeWithTag(BotsTags.host("h1")).performClick()
        compose.onNodeWithTag(BotsTags.host("h2")).performClick()
        assertEquals(listOf("toggleHost:h1:true", "toggleHost:h2:false"), events)
    }

    @Test fun theDetailsButtonOpensTheComputerPageOfThatComputer() {
        showTree(tree(listOf(host("h1"), host("h2"))))
        compose.onNodeWithTag(BotsTags.info("h2")).performClick()
        assertEquals(listOf("openHost:h2"), events)
    }

    @Test fun aCollapsedComputerShowsNoRowsUnderIt() {
        showTree(tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")), collapsed = setOf("h1")))
        compose.onNodeWithTag(BotsTags.host("h1")).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.bot("h1", "a")).assertDoesNotExist()
    }

    // --- a section with nothing to list ---------------------------------------------------------------------------

    @Test fun connectedWithBotsNotYetDeliveredSaysLoading() {
        showTree(tree(listOf(host("h1")), ready("h1")))
        compose.onNodeWithTag(BotsTags.state("h1")).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_loading)).assertIsDisplayed()
    }

    @Test fun anOldDesktopSaysItDoesNotSupportBots() {
        showTree(tree(listOf(host("h1")), ready("h1"), RemoteBotsState("h1", RemoteBotsStateFfi.Unsupported)))
        compose.onNodeWithText(text(R.string.mobile_bots_unsupported)).assertIsDisplayed()
    }

    @Test fun aComputerWithNoBotsSaysSo() {
        showTree(tree(listOf(host("h1")), ready("h1"), liveBots("h1")))
        compose.onNodeWithText(text(R.string.mobile_bots_none)).assertIsDisplayed()
    }

    @Test fun aComputerNeverConnectedOffersToConnect() {
        showTree(tree(listOf(host("h1"))))
        compose.onNodeWithText(text(R.string.mobile_bots_connect_to_load)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.connect("h1")).performClick()
        assertEquals(listOf("connect:h1"), events)
    }

    @Test fun anOfflineComputerWithNothingSavedSaysOfflineAndHowToFixIt() {
        showTree(tree(listOf(host("h1", online = false))))
        compose.onNodeWithText(text(R.string.remote_offline_help)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.connect("h1")).assertDoesNotExist()
    }

    @Test fun anUnpairedComputerOffersToConnectAndPair() {
        showTree(tree(listOf(host("h1", paired = false))))
        compose.onNodeWithText(text(R.string.remote_not_paired)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.connect("h1")).performClick()
        assertEquals(listOf("connect:h1"), events)
    }

    // --- saved rows and their notes -------------------------------------------------------------------------------

    @Test fun savedRowsOfAnOnlineComputerCarryTheSavedNote() {
        showTree(tree(listOf(host("h1")), snapshots = listOf(saved("h1", savedBot("a")))))
        compose.onNodeWithText("Bot a").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.mobile_bots_saved_note_fmt, text(R.string.session_relative_hours_ago_fmt, 3))).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.mobile_bots_offline_chats_note)).assertDoesNotExist()
    }

    @Test fun savedRowsOfAnOfflineComputerSayTheirChatsCannotBeOpened() {
        showTree(tree(listOf(host("h1", online = false)), snapshots = listOf(saved("h1", savedBot("a")))))
        compose.onNodeWithText("Bot a").assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.note("h1", BodyNote.SavedNote)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.mobile_bots_offline_chats_note)).assertIsDisplayed()
    }

    // --- connection notes ---------------------------------------------------------------------------------------------

    @Test fun aComputerBeingDialledSaysSo() {
        showTree(tree(listOf(host("h1", name = "Studio")), connection("h1", RemoteConnectionPhaseFfi.CONNECTING)))
        compose.onNodeWithTag(BotsTags.connection("h1")).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_connecting_fmt, "Studio")).assertIsDisplayed()
    }

    @Test fun aComputerThatWaitsForApprovalShowsTheCodeInsideItsSection() {
        showTree(
            tree(listOf(host("h1", name = "Studio", paired = false)), connection("h1", RemoteConnectionPhaseFfi.PAIRING, code = "123456")),
        )
        compose.onNodeWithText(text(R.string.remote_pairing_title)).assertIsDisplayed()
        compose.onNodeWithText("123 456").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_pairing_waiting)).assertIsDisplayed()
    }

    @Test fun aDroppedLinkSaysItIsReconnecting() {
        showTree(
            tree(listOf(host("h1")), connection("h1", RemoteConnectionPhaseFfi.RECONNECTING, everReady = true), liveBots("h1")),
        )
        compose.onNodeWithText(text(R.string.remote_reconnecting)).assertIsDisplayed()
    }

    @Test fun aFailedConnectionShowsItsMessageAndARetry() {
        val failure = RemoteFailureFfi(RemoteFailureKindFfi.NO_ANSWER, null, false)
        showTree(tree(listOf(host("h1")), connection("h1", RemoteConnectionPhaseFfi.FAILED, failure = failure)))
        compose.onNodeWithText(text(R.string.remote_fail_no_answer)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retryConnection"), events)
    }

    @Test fun aRevokedPairingOffersToPairAgain() {
        val failure = RemoteFailureFfi(RemoteFailureKindFfi.REVOKED, null, false)
        showTree(tree(listOf(host("h1")), connection("h1", RemoteConnectionPhaseFfi.FAILED, failure = failure)))
        compose.onNodeWithText(text(R.string.remote_fail_revoked)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_pair_again)).performClick()
        assertEquals(listOf("repair:h1"), events)
        compose.onNodeWithText(text(R.string.common_retry)).assertDoesNotExist()
    }

    // --- Bot rows ------------------------------------------------------------------------------------------------------

    private val digest = liveBot("b1", "Daily digest", "Sums up the night", RemoteBotStatusFfi.RUNNING, waiting = 2)

    @Test fun aBotRowShowsItsNameDescriptionStatusAndWaitingCount() {
        showTree(tree(listOf(host("h1")), ready("h1"), liveBots("h1", digest)))
        compose.onNodeWithText("Daily digest").assertIsDisplayed()
        compose.onNodeWithText("Sums up the night").assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.BOT_RUNNING, useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_status_running)).assertIsDisplayed()
        compose.onNodeWithContentDescription(text(R.string.remote_bot_waiting_fmt, 2)).assertIsDisplayed()
    }

    @Test fun aBotThatNeedsTheUserSaysSoAndAnIdleOneIsQuiet() {
        showTree(
            tree(
                listOf(host("h1")), ready("h1"),
                liveBots("h1", liveBot("b1", "Needy", status = RemoteBotStatusFfi.REQUIRES_ACTION), liveBot("b2", "Calm")),
            ),
        )
        compose.onNodeWithText(text(R.string.remote_status_requires_action)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_status_idle)).assertIsDisplayed()
    }

    @Test fun aBotThatWaitsOnNobodyShowsNoWaitingBadge() {
        showTree(tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("b1", "Calm"))))
        compose.onNodeWithContentDescription(text(R.string.remote_bot_waiting_fmt, 0)).assertDoesNotExist()
    }

    @Test fun tappingABotAsksToToggleIt() {
        showTree(tree(listOf(host("h1")), ready("h1"), liveBots("h1", digest)))
        compose.onNodeWithTag(BotsTags.bot("h1", "b1")).performClick()
        assertEquals(listOf("toggleBot:h1/b1"), events)
    }

    @Test fun botsAreListedByActivityThenName() {
        showTree(tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a", "Zed"), liveBot("b", "Alpha"))))
        assertTrue(below("Alpha", "Zed"))
    }

    // --- an open Bot ---------------------------------------------------------------------------------------------------

    private fun openTree(chats: RemoteBotChatsState = RemoteBotChatsState(), connection: RemoteConnectionStateFfi = ready("h1")) =
        tree(listOf(host("h1")), connection, liveBots("h1", digest), chats, expanded = setOf("h1/b1"))

    @Test fun anOpenBotOffersNewChatAboveItsChatsInTheHostsOrder() {
        showTree(openTree(heldChats("h1", "b1", listOf(liveChat("s2", "Tuesday run"), liveChat("s1", "Monday run")))))
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertIsEnabled()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertIsDisplayed()
        assertTrue(below(text(R.string.remote_bot_new_chat), "Tuesday run"))
        assertTrue(below("Tuesday run", "Monday run"))
    }

    @Test fun tappingNewChatAsksForOneOnThatBot() {
        showTree(openTree(heldChats("h1", "b1", emptyList())))
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).performClick()
        assertEquals(listOf("newChat:b1"), events)
    }

    @Test fun newChatIsOffWhileTheLinkIsNotReady() {
        showTree(openTree(heldChats("h1", "b1", emptyList()), connection("h1", RemoteConnectionPhaseFfi.RECONNECTING, everReady = true)))
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertIsNotEnabled()
    }

    @Test fun newChatIsOffOnAnOfflineComputer() {
        showTree(
            tree(
                listOf(host("h1", online = false)),
                snapshots = listOf(saved("h1", savedBot("a", chats = listOf(ChatItem("s1", "Saved chat"))))),
                expanded = setOf("h1/a"),
            ),
        )
        compose.onNodeWithTag(BotsTags.newChat("h1", "a")).assertIsNotEnabled()
        compose.onNodeWithText("Saved chat").assertIsDisplayed()
    }

    @Test fun newChatIsOffWhileOneIsBeingMade() {
        showTree(openTree(heldChats("h1", "b1", emptyList())), creating = listOf("h1/b1"))
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertIsNotEnabled()
    }

    @Test fun aChatThatCouldNotBeStartedSaysWhyUnderNewChat() {
        showTree(openTree(heldChats("h1", "b1", emptyList())), errors = mapOf("h1/b1" to R.string.remote_err_capacity))
        compose.onNodeWithTag(BotsTags.chatError("h1", "b1")).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_err_capacity)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertIsEnabled()
    }

    @Test fun anOpenBotWhoseChatsLoadSaysSo() {
        showTree(openTree())
        compose.onNodeWithText(text(R.string.mobile_bots_chats_loading)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertIsEnabled()
    }

    @Test fun anOpenBotWhoseChatsFailedOffersARetry() {
        showTree(openTree(heldChats("h1", "b1", null, RemoteSessionErrorFfi("timeout", "slow"))))
        compose.onNodeWithText(text(R.string.mobile_bots_chats_failed)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retryChats:h1/b1"), events)
    }

    @Test fun aFailedListingKeepsTheChatsThatWereHeld() {
        showTree(openTree(heldChats("h1", "b1", listOf(liveChat("s1", "Monday run")), RemoteSessionErrorFfi("internal", "offline"))))
        compose.onNodeWithText("Monday run").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.mobile_bots_chats_failed)).assertIsDisplayed()
    }

    @Test fun anOpenBotWithNoChatsSaysSo() {
        showTree(openTree(heldChats("h1", "b1", emptyList())))
        compose.onNodeWithText(text(R.string.remote_bot_no_chats)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertIsEnabled()
    }

    @Test fun aBotThatIsGoneSaysSoAndOffersNoNewChat() {
        showTree(openTree(heldChats("h1", "b1", null, RemoteSessionErrorFfi("not_remote_enabled", "off"))))
        compose.onNodeWithText(text(R.string.remote_bot_gone)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.newChat("h1", "b1")).assertDoesNotExist()
    }

    // --- chats ---------------------------------------------------------------------------------------------------------

    @Test fun tappingAChatOpensIt() {
        showTree(openTree(heldChats("h1", "b1", listOf(liveChat("s1", "Monday run")))))
        compose.onNodeWithTag(BotsTags.chat("h1", "b1", "s1")).performClick()
        assertEquals(listOf("openChat:s1"), events)
    }

    @Test fun aBlankChatTitleReadsAsUntitled() {
        showTree(openTree(heldChats("h1", "b1", listOf(liveChat("s1", "")))))
        compose.onNodeWithText(text(R.string.remote_untitled)).assertIsDisplayed()
    }

    @Test fun aChatShowsWhatItWaitsOnOrThatItRuns() {
        showTree(
            openTree(
                heldChats(
                    "h1", "b1",
                    listOf(
                        liveChat("s1", "Asking", RemoteSessionStatusFfi.REQUIRES_ACTION, RemotePendingActionFfi.PERMISSION),
                        liveChat("s2", "Working", RemoteSessionStatusFfi.RUNNING),
                    ),
                ),
            ),
        )
        compose.onNodeWithText(text(R.string.remote_needs_permission)).assertIsDisplayed()
        compose.onNodeWithTag(BotsTags.chat("h1", "b1", "s2")).assertIsDisplayed()
    }

    // --- the window of chats -------------------------------------------------------------------------------------------

    private fun manyChats(count: Int) = (1..count).map { liveChat("c$it", "Run $it") }

    @Test fun aBotShowsFiveChatsThenOffersTheRest() {
        showTree(openTree(heldChats("h1", "b1", manyChats(6))))
        compose.onNodeWithText("Run 5").assertIsDisplayed()
        compose.onNodeWithText("Run 6").assertDoesNotExist()
        compose.onNodeWithText(text(R.string.mobile_bots_show_more_fmt, 1)).performClick()
        assertEquals(listOf("more:h1/b1"), events)
    }

    @Test fun showingAllOffersToShowFewer() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", digest), heldChats("h1", "b1", manyChats(6)),
            expanded = setOf("h1/b1"), showAll = setOf("h1/b1"),
        )
        showTree(rows)
        compose.onNodeWithText("Run 6").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.chat_tool_show_less)).performClick()
        assertEquals(listOf("less:h1/b1"), events)
    }

    @Test fun fiveChatsOfferNothingMore() {
        showTree(openTree(heldChats("h1", "b1", manyChats(5))))
        compose.onNodeWithTag(BotsTags.showMore("h1", "b1")).assertDoesNotExist()
        compose.onNodeWithTag(BotsTags.showLess("h1", "b1")).assertDoesNotExist()
    }
}

private const val HOUR = 3_600_000L
private const val DAY = 24 * HOUR
