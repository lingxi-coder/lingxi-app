package com.lingxi.code.teams

import android.content.res.Configuration
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import com.lingxi.code.testing.finishComposeFixture
import com.lingxi.code.theme.LingXiTheme
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Locale

/**
 * The run card, the Bot card and the empty state of docs/TEAMS-UI-POLISH.md (U4, U5, U8, U9): one anatomy for every state (the state as a word, the
 * Bot's name, one line of detail, at most one button), the usage line, the one Stop sharing dialog, and a message state that names what is empty.
 */
@RunWith(AndroidJUnit4::class)
class TeamBotComponentsUiTest {
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private val events = mutableListOf<String>()
    private fun text(id: Int, vararg args: Any): String = rule.activity.getString(id, *args)

    private val me = TeamMemberRef("u1", 1)

    private fun task(
        state: String, usage: TeamTaskUsage? = null, artifactId: String? = null, wait: TeamTaskWait? = null,
    ) = TeamTask("task1", "c1", "sharedbot_helper", me, state, 0L, 0L, null, artifactId, usage, wait = wait)

    /**
     * The card for the task in [current]; the number format is pinned to en-US, the words come from the device's own resources. [viewer] is who looks
     * at it (nobody by default, so no member action is offered) and [onReply] is set where the card is the Bot's question.
     */
    private fun showCard(first: TeamTask, viewer: TeamMemberRef? = null, onReply: (() -> Unit)? = null) {
        var current by mutableStateOf(first)
        rule.setContent {
            val config = Configuration(LocalConfiguration.current).apply { setLocale(Locale.US) }
            CompositionLocalProvider(LocalConfiguration provides config) {
                LingXiTheme(darkTheme = false) {
                    Column {
                        TeamTaskCard(
                            current, "Helper", onArtifact = { events += "artifact:$it" }, onRefresh = { events += "refresh:$it" },
                            viewer = viewer, recipientName = "Mia", onCancel = { events += "cancel:$it" }, onReply = onReply,
                        )
                    }
                }
            }
        }
        stateOfCard = { next -> rule.runOnIdle { current = next }; rule.waitForIdle() }
        rule.waitForIdle()
    }

    private var stateOfCard: (TeamTask) -> Unit = {}

    private fun buttonsOfCard() = rule.onAllNodes(hasClickAction() and hasAnyAncestor(hasTestTag("teams.task.task1")))

    /** The one merged element of the card's text: the state, the Bot's name and the detail are read together. */
    private fun cardText(state: Int, detail: Int?) {
        rule.onNode(hasText(text(state)) and hasText("Helper") and (detail?.let { hasText(text(it)) } ?: hasText(text(state)))).assertIsDisplayed()
    }

    // --- U4: one anatomy ---------------------------------------------------------------------------------------------

    @Test fun everyStateIsOneElementOfStateNameAndDetailWithAtMostOneButton() {
        showCard(task("queued"))
        // (state, label, detail, a button's label or null)
        val table = listOf(
            Triple("queued", R.string.teams_run_queued to R.string.teams_run_queued_detail, R.string.teams_run_refresh),
            Triple("running", R.string.teams_run_processing to R.string.teams_run_processing_detail, R.string.teams_run_refresh),
            Triple("failed", R.string.chat_status_failed to R.string.teams_run_failed_detail, null),
            Triple("expired", R.string.teams_run_expired to R.string.teams_run_expired_detail, null),
            Triple("interrupted", R.string.teams_run_interrupted to R.string.teams_run_interrupted_detail, null),
            Triple("authorization_required", R.string.teams_run_authorization_required to R.string.teams_run_authorization_required_detail, R.string.teams_run_refresh),
            Triple("revoked", R.string.teams_run_revoked to R.string.teams_run_revoked_detail, null),
        )
        for ((state, words, button) in table) {
            stateOfCard(task(state))
            cardText(words.first, words.second)
            if (button == null) buttonsOfCard().assertCountEquals(0)
            else {
                buttonsOfCard().assertCountEquals(1)
                rule.onNode(hasText(text(button)) and hasClickAction()).assertIsDisplayed()
            }
        }
    }

    // --- Teams M2: the states and actions main added ------------------------------------------------------------------

    @Test fun aCancelledRunReadsCancelledHasNoButtonAndShowsUsageOnlyWhenTheBotGotAsFarAsAModelCall() {
        showCard(task("cancelled"))
        rule.onNode(hasText(text(R.string.teams_task_cancelled)) and hasText("Helper")).assertIsDisplayed()
        buttonsOfCard().assertCountEquals(0)
        usageLine().assertDoesNotExist()
        stateOfCard(task("cancelled", TeamTaskUsage("complete", 10, 5, null, null)))
        usageLine().assert(hasText(text(R.string.teams_usage_tokens_total_fmt, "15") + " · " + text(R.string.teams_usage_cost_unknown)))
    }

    @Test fun aTaskThatNeedsCheckingSaysSoInItsOwnWordsAndOffersRefreshOnly() {
        showCard(task("needs_reconciliation"))
        rule.onNode(hasText(text(R.string.teams_task_needs_checking)) and hasText("Helper") and hasText(text(R.string.teams_task_needs_checking_hint))).assertIsDisplayed()
        buttonsOfCard().assertCountEquals(1)
        rule.onNode(hasText(text(R.string.teams_run_refresh)) and hasClickAction()).assertIsDisplayed()
    }

    @Test fun aWaitingTaskSaysWhoItWaitsForAndTheMemberCanAnswerCancelOrRefreshIt() {
        val waiting = task("waiting", wait = TeamTaskWait("wait1", "human_input", me, System.currentTimeMillis() + 4 * 60_000L, "m1"))
        showCard(waiting, viewer = me, onReply = { events += "reply" })
        rule.onNode(hasText(text(R.string.teams_task_waiting)) and hasText("Helper")).assertIsDisplayed()
        rule.onNodeWithTag("teams.task.wait.task1", useUnmergedTree = true).assertIsDisplayed()
        buttonsOfCard().assertCountEquals(3)
        rule.onNode(hasText(text(R.string.teams_message_reply)) and hasClickAction()).performClick()
        rule.onNode(hasText(text(R.string.teams_task_cancel)) and hasClickAction()).performClick()
        assertEquals(listOf("reply", "cancel:task1"), events)
    }

    @Test fun aStateThisAppDoesNotKnowReadsUnknownWithoutADetailOrAButton() {
        showCard(task("something_new"))
        rule.onNode(hasText(text(R.string.chat_status_unknown)) and hasText("Helper")).assertIsDisplayed()
        buttonsOfCard().assertCountEquals(0)
    }

    @Test fun refreshStatusAsksForThisTaskOnly() {
        showCard(task("queued"))
        rule.onNode(hasText(text(R.string.teams_run_refresh)) and hasClickAction()).performClick()
        assertEquals(listOf("refresh:task1"), events)
    }

    @Test fun aFinishedRunWithADocumentOffersOpenDocumentAndAFailedOneOffersNothing() {
        showCard(task("completed", TeamTaskUsage("complete", 10, 5, 1_322.0, "USD"), artifactId = "art1"))
        buttonsOfCard().assertCountEquals(1)
        rule.onNode(hasText(text(R.string.teams_artifact_open)) and hasClickAction()).performClick()
        assertEquals(listOf("artifact:art1"), events)
        stateOfCard(task("failed", artifactId = "art1"))
        buttonsOfCard().assertCountEquals(0)
    }

    @Test fun aFinishedRunWithoutADocumentHasNoButton() {
        showCard(task("completed", TeamTaskUsage("complete", 10, 5, 1_322.0, "USD")))
        buttonsOfCard().assertCountEquals(0)
    }

    // --- U5: the usage line ------------------------------------------------------------------------------------------

    /** The detail line on its own: inside the card's merged text element its tag is merged away, so it is read from the unmerged tree. */
    private fun usageLine() = rule.onNodeWithTag("teams.task.usage.task1", useUnmergedTree = true)

    @Test fun aCompletedRunSaysTotalTokensThenACostWithTwoToFourDecimals() {
        showCard(task("completed", TeamTaskUsage("complete", 1_000, 234, 1_322.0, "USD")))
        usageLine().assert(hasText(text(R.string.teams_usage_tokens_total_fmt, "1.2K") + " · $0.0013"))
    }

    @Test fun theSplitIsTheSpokenValueOfTheCardText() {
        showCard(task("completed", TeamTaskUsage("complete", 1_000, 234, 1_322.0, "USD")))
        val split = text(R.string.teams_usage_tokens_fmt, 1_000L, 234L)
        rule.onNode(hasText(text(R.string.chat_status_completed)) and hasText("Helper"))
            .assert(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, split))
    }

    @Test fun aPartialRunPutsPartialUsageInFront() {
        showCard(task("completed", TeamTaskUsage("partial", 10, 5, 1_322.0, "USD")))
        usageLine().assert(hasText(text(R.string.teams_usage_partial) + " · " + text(R.string.teams_usage_tokens_total_fmt, "15") + " · $0.0013"))
    }

    @Test fun aMissingUsageSaysUnavailableAndAnUnpricedOneSaysCostUnknown() {
        showCard(task("completed", TeamTaskUsage("missing")))
        usageLine().assert(hasText(text(R.string.teams_usage_missing)))
        stateOfCard(task("completed", TeamTaskUsage("complete", 10, 5, null, null)))
        usageLine().assert(hasText(text(R.string.teams_usage_tokens_total_fmt, "15") + " · " + text(R.string.teams_usage_cost_unknown)))
    }

    @Test fun aTinyCostReadsLessThanTheSmallestAmount() {
        showCard(task("completed", TeamTaskUsage("complete", 10, 5, 20.0, "USD")))
        usageLine().assert(hasText(text(R.string.teams_usage_tokens_total_fmt, "15") + " · <$0.0001"))
    }

    // --- U8: the one Stop sharing dialog ------------------------------------------------------------------------------

    private val bot = TeamBotCard(
        "sharedbot_helper", "Helper", "", TeamMemberRef("u1", 1), 1, "published", true, true, 0,
        JSONObject().put("mode", "all"), JSONObject().put("team", 100).put("member", 100).put("bot", 100), null,
    )

    private fun inDialog(label: String) = hasText(label) and hasAnyAncestor(isDialog())

    @Test fun stopSharingNamesTheBotSaysWhatStopsAndWhatStaysAndCancelChangesNothing() {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                TeamBotCardView(bot, "Me", "u1", 1, enabled = true, onMessage = { events += "message" }, onStopSharing = { events += "stop" })
            }
        }
        rule.onNodeWithText(text(R.string.friends_share_bot_stop)).performClick()
        rule.onNode(inDialog(text(R.string.teams_bot_stop_title_fmt, "Helper"))).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.teams_bot_stop_detail))).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.common_cancel))).performClick()
        rule.waitForIdle()
        assertEquals(emptyList<String>(), events)
        rule.onNodeWithText(text(R.string.friends_share_bot_stop)).performClick()
        rule.onNode(inDialog(text(R.string.friends_share_bot_stop)) and hasClickAction()).performClick()
        rule.waitForIdle()
        assertEquals(listOf("stop"), events)
    }

    @Test fun anotherPersonsBotOffersNoStopSharing() {
        rule.setContent { LingXiTheme(darkTheme = false) { TeamBotCardView(bot, "Ann", "u2", 1, enabled = true, onMessage = {}, onStopSharing = {}) } }
        rule.onNodeWithText(text(R.string.friends_share_bot_stop)).assertDoesNotExist()
    }

    // --- U9: an empty state says what is empty and what to do ---------------------------------------------------------

    @Test fun aMessageStateHasAHeadingABodyAndItsAction() {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                TeamsMessageState("Nothing here", body = "Say hello.", fill = false, action = { Button(onClick = { events += "act" }) { Text("Go") } })
            }
        }
        rule.onNodeWithText("Nothing here").assert(SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading))
        rule.onNodeWithText("Say hello.").assertIsDisplayed()
        rule.onNodeWithText("Go").performClick()
        assertEquals(listOf("act"), events)
    }
}
