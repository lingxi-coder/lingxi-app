package com.lingxi.code.remote

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.getBoundsInRoot
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The Bot section of a computer's screen and the page of one Bot, in each state. The state is
 * decided by the pure functions the JVM tests cover; these show what is drawn for it.
 */
@RunWith(AndroidJUnit4::class)
class RemoteBotsUiTest {
    @get:Rule val compose = createComposeRule()

    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private fun text(id: Int, vararg args: Any) = context.getString(id, *args)

    private fun bot(
        id: String,
        name: String,
        description: String = "",
        status: RemoteBotStatusFfi = RemoteBotStatusFfi.IDLE,
        waiting: UInt = 0u,
    ) = RemoteBotSummaryFfi(id, name, description, status, waiting, null)

    private fun session(id: String, title: String) =
        RemoteSessionSummaryFfi(id, title, "", RemoteSessionStatusFfi.IDLE, null, null, 0u)

    private fun show(content: @Composable () -> Unit) = compose.setContent { LingXiTheme(darkTheme = false) { content() } }

    private val digest = bot("b1", "Daily digest", "Sums up the night", RemoteBotStatusFfi.RUNNING, waiting = 2u)

    private fun page(
        page: RemoteBotPageUi,
        note: Int? = null,
        creating: Boolean = false,
        createError: Int? = null,
        onBack: () -> Unit = {},
        onNewChat: () -> Unit = {},
        onRetry: () -> Unit = {},
        onOpenChat: (RemoteSessionSummaryFfi) -> Unit = {},
    ) = show { RemoteBotPage(page, note, creating, createError, onBack, onNewChat, onRetry, onOpenChat) }

    // --- the section on the computer's screen -------------------------------

    @Test fun botsAreListedAboveTheSessionsWithTheirStatusAndWaitingCount() {
        val opened = mutableListOf<String>()
        show {
            RemoteSessionList(
                transport = null,
                bots = listOf(digest, bot("b2", "Inbox triage")),
                sessions = listOf(session("s1", "Refactor the parser")),
                onOpenBot = { opened += it.botId },
                onOpenSession = {},
            )
        }
        compose.onNodeWithText(text(R.string.remote_bots)).assertIsDisplayed()
        compose.onNodeWithText("Sums up the night").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_waiting_fmt, 2)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_status_running)).assertIsDisplayed()
        compose.onNodeWithText("Inbox triage").assertIsDisplayed()
        assertTrue(
            compose.onNodeWithText("Inbox triage").getBoundsInRoot().bottom <=
                compose.onNodeWithText("Refactor the parser").getBoundsInRoot().top,
        )
        compose.onNodeWithText("Daily digest").performClick()
        assertEquals(listOf("b1"), opened)
    }

    @Test fun aBotThatWaitsOnNobodyShowsNoWaitingTag() {
        show {
            RemoteSessionList(null, listOf(bot("b2", "Inbox triage")), emptyList(), onOpenBot = {}, onOpenSession = {})
        }
        compose.onNodeWithText("Inbox triage").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_waiting_fmt, 0)).assertDoesNotExist()
    }

    @Test fun aComputerWithoutBotsLooksAsItAlwaysDid() {
        show {
            RemoteSessionList(null, emptyList(), listOf(session("s1", "Refactor the parser")), onOpenBot = {}, onOpenSession = {})
        }
        compose.onNodeWithText(text(R.string.remote_bots)).assertDoesNotExist()
        compose.onNodeWithText("Refactor the parser").assertIsDisplayed()
    }

    @Test fun sessionsStillSayWhenThereAreNoneAndWhileTheyLoad() {
        show { RemoteSessionList(null, emptyList(), null, onOpenBot = {}, onOpenSession = {}) }
        compose.onNodeWithText(text(R.string.remote_loading)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_no_sessions)).assertDoesNotExist()
    }

    @Test fun anEmptyComputerSaysSoBelowItsBots() {
        show { RemoteSessionList(null, listOf(digest), emptyList(), onOpenBot = {}, onOpenSession = {}) }
        compose.onNodeWithText("Daily digest").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_no_sessions)).assertIsDisplayed()
    }

    // --- the page of one Bot -------------------------------------------------

    @Test fun theBotPageTitlesItselfWithTheBotsNameAndDescribesIt() {
        page(RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(threads = emptyList(), failed = false)))
        compose.onNodeWithText("Daily digest").assertIsDisplayed()
        compose.onNodeWithText("Sums up the night").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_waiting_fmt, 2)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_status_running)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertIsEnabled()
    }

    @Test fun theBotPageWaitsWhileTheBotsAreNotKnown() {
        page(RemoteBotPageUi.Loading)
        compose.onNodeWithText(text(R.string.remote_loading)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertDoesNotExist()
    }

    @Test fun theBotPageLoadsItsChats() {
        page(RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(threads = null, failed = false)))
        compose.onNodeWithText(text(R.string.remote_loading)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_no_chats)).assertDoesNotExist()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertIsEnabled()
    }

    @Test fun theBotPageSaysThereAreNoChatsYet() {
        page(RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(threads = emptyList(), failed = false)))
        compose.onNodeWithText(text(R.string.remote_bot_no_chats)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_loading)).assertDoesNotExist()
    }

    @Test fun theBotPageListsItsChatsNewestFirstAndOpensOne() {
        val opened = mutableListOf<String>()
        page(
            RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(listOf(session("s2", "Tuesday run"), session("s1", "Monday run")), failed = false)),
            onOpenChat = { opened += it.sessionId },
        )
        assertTrue(
            compose.onNodeWithText("Tuesday run").getBoundsInRoot().top < compose.onNodeWithText("Monday run").getBoundsInRoot().top,
        )
        compose.onNodeWithText(text(R.string.remote_bot_no_chats)).assertDoesNotExist()
        compose.onNodeWithText("Monday run").performClick()
        assertEquals(listOf("s1"), opened)
    }

    @Test fun startingAChatIsOfferedAndTakenOnce() {
        var started = 0
        page(RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(emptyList(), failed = false)), onNewChat = { started++ })
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).performClick()
        assertEquals(1, started)
    }

    @Test fun theNewChatButtonIsOffWhileACreateIsInFlight() {
        page(RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(emptyList(), failed = false)), creating = true)
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertIsNotEnabled()
    }

    @Test fun theNewChatButtonIsOffWhileTheComputerIsNotConnected() {
        page(RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(emptyList(), failed = false)), note = R.string.remote_reconnecting)
        compose.onNodeWithText(text(R.string.remote_reconnecting)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertIsNotEnabled()
    }

    @Test fun aChatThatCouldNotBeStartedSaysWhyBesideTheButton() {
        page(
            RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(emptyList(), failed = false)),
            createError = R.string.remote_err_capacity,
        )
        compose.onNodeWithText(text(R.string.remote_err_capacity)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertIsEnabled()
    }

    @Test fun aFailedListingOffersARetryAndKeepsWhatWasHeld() {
        var retried = 0
        page(
            RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(listOf(session("s1", "Monday run")), failed = true)),
            onRetry = { retried++ },
        )
        compose.onNodeWithText(text(R.string.remote_err_generic)).assertIsDisplayed()
        compose.onNodeWithText("Monday run").assertIsDisplayed()
        compose.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(1, retried)
    }

    @Test fun aBotThatIsGoneSaysSoAndLeadsBack() {
        var back = 0
        page(RemoteBotPageUi.Gone, onBack = { back++ })
        compose.onNodeWithText(text(R.string.remote_bot_gone)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_new_chat)).assertDoesNotExist()
        compose.onNodeWithText(text(R.string.remote_back)).performClick()
        assertEquals(1, back)
    }

    @Test fun aBotPageIsNeverBlankWhateverBecomesOfTheBot() {
        // The title stands in for a Bot that is not known, and it stays through the changes of state.
        var state by mutableStateOf<RemoteBotPageUi>(RemoteBotPageUi.Loading)
        show { RemoteBotPage(state, null, false, null, {}, {}, {}, {}) }
        compose.onNodeWithText(text(R.string.remote_bots)).assertIsDisplayed()
        compose.runOnIdle { state = RemoteBotPageUi.Shown(digest, RemoteBotChatsUi(emptyList(), failed = false)) }
        compose.onNodeWithText("Daily digest").assertIsDisplayed()
        compose.runOnIdle { state = RemoteBotPageUi.Gone }
        compose.onNodeWithText(text(R.string.remote_bots)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_bot_gone)).assertIsDisplayed()
    }
}
