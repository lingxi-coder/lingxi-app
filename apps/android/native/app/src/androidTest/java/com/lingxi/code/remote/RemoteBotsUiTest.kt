package com.lingxi.code.remote

import androidx.compose.runtime.Composable
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.getBoundsInRoot
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.material3.Text
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import com.lingxi.code.testing.finishComposeFixture
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * What is left of the Remote screens once the Bots live in the Bots tab's tree (SPEC-mobile-team-bot-ui section 4.6):
 * the Computer page's shared-session list, the page frame the tab pushes it in, and the sign-in gate. The Bots
 * themselves are drawn and tested in `bots/BotsTabUiTest`.
 */
@RunWith(AndroidJUnit4::class)
class RemoteBotsUiTest {
    @After fun closeFixture() = finishComposeFixture(compose.activity)

    @get:Rule val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private fun text(id: Int, vararg args: Any) = context.getString(id, *args)

    private fun session(
        id: String,
        title: String,
        status: RemoteSessionStatusFfi = RemoteSessionStatusFfi.IDLE,
        pending: RemotePendingActionFfi? = null,
    ) = RemoteSessionSummaryFfi(id, title, "project", status, pending, null, 0u)

    private fun show(content: @Composable () -> Unit) = compose.setContent { LingXiTheme(darkTheme = false) { content() } }

    // --- the Computer page's session list ---------------------------------------

    @Test fun theComputerPageListsItsSessionsAndNoBots() {
        show {
            RemoteSessionList(
                transport = null,
                sessions = listOf(session("s1", "Refactor the parser"), session("s2", "Write the docs")),
                onOpenSession = {},
            )
        }
        compose.onNodeWithText("Refactor the parser").assertIsDisplayed()
        compose.onNodeWithText("Write the docs").assertIsDisplayed()
        // The Bots are the tree's now: the page no longer has a section for them.
        compose.onNodeWithText(text(R.string.remote_bots)).assertDoesNotExist()
        assertTrue(
            compose.onNodeWithText("Refactor the parser").getBoundsInRoot().top <
                compose.onNodeWithText("Write the docs").getBoundsInRoot().top,
        )
    }

    @Test fun aSessionOpensWhenItIsTapped() {
        val opened = mutableListOf<String>()
        show { RemoteSessionList(null, listOf(session("s1", "Refactor the parser")), onOpenSession = { opened += it.sessionId }) }
        compose.onNodeWithText("Refactor the parser").performClick()
        assertEquals(listOf("s1"), opened)
    }

    @Test fun aSessionShowsItsStatusAndWhatItWaitsOn() {
        show {
            RemoteSessionList(
                null,
                listOf(
                    session("s1", "Running one", RemoteSessionStatusFfi.RUNNING),
                    session("s2", "Waiting one", RemoteSessionStatusFfi.REQUIRES_ACTION, RemotePendingActionFfi.QUESTION),
                ),
                onOpenSession = {},
            )
        }
        compose.onNodeWithText(text(R.string.remote_status_running)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_needs_question)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_status_requires_action)).assertIsDisplayed()
    }

    @Test fun aBlankSessionTitleReadsAsUntitled() {
        show { RemoteSessionList(null, listOf(session("s1", "")), onOpenSession = {}) }
        compose.onNodeWithText(text(R.string.remote_untitled)).assertIsDisplayed()
    }

    @Test fun sessionsSayWhileTheyLoadAndWhenThereAreNone() {
        show { RemoteSessionList(null, null, onOpenSession = {}) }
        compose.onNodeWithText(text(R.string.remote_loading)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_no_sessions)).assertDoesNotExist()
    }

    @Test fun anEmptyComputerSaysSoWithItsHelp() {
        show { RemoteSessionList(null, emptyList(), onOpenSession = {}) }
        compose.onNodeWithText(text(R.string.remote_no_sessions)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_no_sessions_help)).assertIsDisplayed()
    }

    @Test fun theTransportIsExplainedAboveTheSessions() {
        show { RemoteSessionList(RemoteTransportFfi.WEBRTC, emptyList(), onOpenSession = {}) }
        compose.onNodeWithText(text(R.string.remote_direct_help)).assertIsDisplayed()
    }

    @Test fun theSessionsHeadingIsShownWhenTheAppsAreAboveIt() {
        show { RemoteSessionList(null, emptyList(), onOpenSession = {}, sessionsTitle = text(R.string.remote_sessions_title)) }
        compose.onNodeWithText(text(R.string.remote_sessions_title)).assertIsDisplayed()
    }

    // --- the page frame -------------------------------------------------------------------

    @Test fun aPageHasATitleABackArrowAndItsContent() {
        var back = 0
        show { RemotePage(title = "Studio Mac", onBack = { back++ }) { Text("page body") } }
        compose.onNodeWithText("Studio Mac").assertIsDisplayed()
        compose.onNodeWithText("page body").assertIsDisplayed()
        compose.onNodeWithContentDescription(text(R.string.onboarding_back)).performClick()
        assertEquals(1, back)
    }

    // --- the sign-in gate --------------------------------------------------------------------

    @Test fun signedOutAsksForTheAccountAndLeadsToIt() {
        var opened = 0
        show { RemoteSignedOut(onOpenAccount = { opened++ }) }
        compose.onNodeWithText(text(R.string.remote_signed_out_title)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_signed_out_body)).assertIsDisplayed()
        compose.onNodeWithText(text(R.string.remote_sign_in)).performClick()
        assertEquals(1, opened)
    }

    @Test fun theSessionListCarriesItsTagsForTheTabsTests() {
        show { RemoteSessionList(null, listOf(session("s1", "One")), onOpenSession = {}) }
        compose.onNodeWithTag(RemoteTags.HOST_PAGE).assertIsDisplayed()
        compose.onNodeWithTag(RemoteTags.SESSIONS).assertIsDisplayed()
    }
}
