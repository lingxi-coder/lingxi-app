package com.lingxi.code.teams

import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.core.app.ActivityScenario
import androidx.test.espresso.Espresso
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.Source
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.tasks.await
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import java.io.File

/** Requires freshly seeded demo Auth/Firestore + the real Teams API fixture. */
class TeamsFeatureAcceptanceTest {
    @get:Rule val compose = createEmptyComposeRule()
    private lateinit var activity: ProbeActivity
    private val answerText = "Android native answer"
    private fun state() = requireNotNull(activity.messagesModel).state.value
    private fun waitFor(predicate: () -> Boolean) = compose.waitUntil(timeoutMillis = 45000, condition = predicate)
    private fun scrollTo(matcher: SemanticsMatcher) {
        compose.onAllNodes(hasScrollAction()).onFirst().performScrollToNode(matcher)
    }
    private fun clickControl(label: String) {
        Espresso.closeSoftKeyboard()
        compose.waitForIdle()
        val matcher = hasText(label) and hasClickAction()
        scrollTo(matcher)
        compose.onNode(matcher).performScrollTo().assertIsDisplayed().assertIsEnabled()
            .performSemanticsAction(SemanticsActions.OnClick) { click -> assertTrue(click()) }
    }
    private fun textField(label: String) = compose.onNode(hasSetTextAction() and hasText(label))
    private fun capture(name: String) {
        val file = File(requireNotNull(activity.getExternalFilesDir("acceptance")), "$name.png")
        file.outputStream().use { compose.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) }
    }
    private fun command(body: JSONObject): JSONObject = runBlocking {
        val token = requireNotNull(FirebaseAuth.getInstance().currentUser?.getIdToken(false)?.await()?.token)
        val cache = requireNotNull(state().cache)
        activity.request("POST", "/v1/teams/messages", body.put("teamId", cache.teamId).put("generation", cache.generation), token)
    }

    private fun recordFailure(failure: Throwable) {
        val directory = requireNotNull(activity.getExternalFilesDir("acceptance"))
        val evidenceErrors = mutableListOf<String>()
        // Preserve the original assertion even if one evidence surface is unavailable.
        fun save(name: String, action: () -> Unit) {
            try { action() } catch (error: Throwable) { evidenceErrors += "$name: ${error.javaClass.simpleName}" }
        }
        save("screenshot") {
            val image = requireNotNull(compose.onRoot().captureToImage().asAndroidBitmap().copy(Bitmap.Config.ARGB_8888, true))
            val canvas = Canvas(image)
            val paint = Paint().apply { color = Color.BLACK }
            val redact = compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).fetchSemanticsNodes() +
                compose.onAllNodes(hasText(answerText, substring = true), useUnmergedTree = true).fetchSemanticsNodes()
            for (node in redact) {
                val bounds = node.boundsInRoot
                canvas.drawRect(bounds.left, bounds.top, bounds.right, bounds.bottom, paint)
            }
            File(directory, "failure.png").outputStream().use { image.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        for (unmerged in listOf(false, true)) save("semantics-$unmerged") {
            val tree = compose.onRoot(useUnmergedTree = unmerged).printToString().replace(answerText, "<redacted-answer>")
            File(directory, if (unmerged) "failure-semantics-unmerged.txt" else "failure-semantics-merged.txt").writeText(tree)
        }
        val snapshot = activity.messagesModel?.state?.value
        val selectedThread = snapshot?.cache?.threads?.get(snapshot.selected)
        val diagnostic = JSONObject().put("failureType", failure.javaClass.simpleName).put("selected", snapshot?.selected)
            .put("offline", snapshot?.offline).put("revoked", snapshot?.revoked)
            .put("threadMessageCount", selectedThread?.messages?.size ?: 0)
            .put("threadQuestionIds", JSONArray(selectedThread?.messages?.mapNotNull { it.question?.id } ?: emptyList<String>()))
            .put("runs", JSONArray(snapshot?.botRuns?.map { (trigger, run) -> JSONObject().put("triggerId", trigger).put("questionId", run.questionId).put("state", run.state).put("answered", run.answered) } ?: emptyList<JSONObject>()))
            .put("pendingAnswerCount", snapshot?.cache?.answers?.size ?: 0).put("pendingSettlementCount", snapshot?.cache?.settlements?.size ?: 0)
            .put("evidenceErrors", JSONArray(evidenceErrors))
        File(directory, "failure-state.json").writeText(diagnostic.toString(2))
    }

    @Test fun realSdkAuthFirestoreAndActualComposeBotConversationTargetsAndQuestion() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val arguments = InstrumentationRegistry.getArguments()
        val uid = arguments.getString("fixture_uid") ?: "android-human"
        val intent = Intent(instrumentation.targetContext, ProbeActivity::class.java)
            .putExtra("fixture_uid", uid).putExtra("fixture_port", (arguments.getString("fixture_port") ?: "18783").toInt())
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ActivityScenario.launch<ProbeActivity>(intent).use { scenario ->
            scenario.onActivity { activity = it }
            try {
                waitFor { activity.messagesModel?.state?.value?.let { !it.offline && it.bots.any(TeamBotCard::selectable) } == true }
                assertEquals(uid, FirebaseAuth.getInstance().currentUser?.uid)
                val cache = requireNotNull(state().cache)
                // This is an actual rules-authorized SDK server read, independent of HTTP.
                val membership = runBlocking { FirebaseFirestore.getInstance().collection("teams").document(cache.teamId).collection("members").document(uid).get(Source.SERVER).await() }
                assertTrue(membership.exists()); assertEquals(cache.generation, membership.getLong("generation"))
                compose.onNodeWithText("Team Bots").assertExists()
                capture("01-bot-cards")

                // Discover a live original-human question supplied by the fixture.
                var questionConversation: String? = null
                var question: TeamMessage? = null
                for (conversation in state().cache!!.conversations) {
                    val page = command(JSONObject().put("action", "sync").put("conversationId", conversation.id).put("after", 0))
                    for (candidate in page.getJSONArray("messages").objects().map(TeamMessage::parse)) {
                        if (candidate.question == null || candidate.originUid != uid || candidate.originGeneration != cache.generation) continue
                        val run = TeamBotRun.parse(command(teamBotStateQuery(conversation.id, requireNotNull(candidate.triggerId))))
                        if (run.canAnswer(candidate, uid, cache.generation, System.currentTimeMillis())) { questionConversation = conversation.id; question = candidate; break }
                    }
                    if (question != null) break
                }
                assertNotNull("Seed one live original-human question immediately before this UI run", question)
                val pendingQuestion = requireNotNull(question)
                scenario.onActivity { it.messagesModel!!.select(requireNotNull(questionConversation)) }
                waitFor {
                    val current = state()
                    val renderedQuestion = current.cache?.threads?.get(questionConversation)?.messages?.find {
                        it.id == pendingQuestion.id && it.question?.id == pendingQuestion.question?.id && it.originUid == uid && it.originGeneration == cache.generation
                    }
                    current.selected == questionConversation && !current.offline && renderedQuestion != null &&
                        current.botRuns[renderedQuestion.triggerId]?.canAnswer(renderedQuestion, uid, cache.generation, System.currentTimeMillis()) == true
                }
                compose.waitForIdle()
                val botQuestion = requireNotNull(pendingQuestion.question)
                for (prompt in botQuestion.questions) {
                    scrollTo(hasSetTextAction() and hasText(prompt)); textField(prompt).performTextInput(answerText)
                }
                scrollTo(hasText("Answer") and hasClickAction())
                capture("02-original-human-question")
                clickControl("Answer")
                waitFor {
                    state().cache!!.answers.isEmpty() &&
                        command(teamBotStateQuery(requireNotNull(questionConversation), requireNotNull(pendingQuestion.triggerId), botQuestion.id)).getBoolean("answered")
                }

                val finalReplyText = "Current acceptance result completed once."
                waitFor {
                    val current = state()
                    val replies = current.cache?.threads?.get(questionConversation)?.messages.orEmpty().count {
                        it.authorKind == "bot" && it.kind == "text" && it.triggerId == pendingQuestion.triggerId && it.text == finalReplyText
                    }
                    current.selected == questionConversation && replies == 1 &&
                        command(teamBotStateQuery(requireNotNull(questionConversation), requireNotNull(pendingQuestion.triggerId), botQuestion.id)).getString("state") == "completed"
                }

                // Create a Bot DM through the actual card, not a copied screen.
                val before = state().cache!!.conversations.map { it.id }.toSet()
                scrollTo(hasText("Message Bot") and isEnabled())
                compose.onAllNodes(hasText("Message Bot") and isEnabled()).onFirst().performScrollTo().assertIsDisplayed()
                    .performSemanticsAction(SemanticsActions.OnClick) { click -> assertTrue(click()) }
                waitFor { state().cache!!.conversations.any { it.id !in before && it.kind == "bot_dm" } }
                val dm = state().cache!!.conversations.first { it.id !in before && it.kind == "bot_dm" }
                assertEquals(1, dm.botKeys.size)
                assertTrue(dm.participants.contains("user:$uid"))
                waitFor { state().selected == dm.id && state().cache!!.creation == null }

                // Group creation includes a Bot participant selected in the real form.
                clickControl("New conversation")
                scrollTo(hasSetTextAction() and hasText("Title"))
                textField("Title").performTextInput("Android T3 acceptance group")
                clickControl("Group")
                compose.onNode(hasText("Group") and hasClickAction()).assertIsSelected()
                scrollTo(hasText("Bot participants (up to 8)"))
                compose.onAllNodes(isToggleable()).onLast().performScrollTo().assertIsDisplayed()
                    .performSemanticsAction(SemanticsActions.OnClick) { click -> assertTrue(click()) }.assertIsOn()
                clickControl("Create conversation")
                waitFor { state().cache!!.conversations.any { it.title == "Android T3 acceptance group" } }
                val group = state().cache!!.conversations.first { it.title == "Android T3 acceptance group" }
                assertEquals("group", group.kind); assertEquals(1, group.botKeys.size)
                waitFor { state().selected == group.id && state().cache!!.creation == null }

                // Plain text @mentions preserve a plain message. The explicit UI target
                // is separately selected for the following message.
                scrollTo(hasSetTextAction() and hasText("Message"))
                textField("Message").performTextInput("@Bot display metadata only")
                clickControl("Send")
                waitFor { state().cache!!.outbox.isEmpty() && state().cache!!.threads[group.id]?.messages?.any { it.text == "@Bot display metadata only" } == true }
                assertTrue(state().cache!!.threads[group.id]!!.messages.first { it.text == "@Bot display metadata only" }.targets.isEmpty())
                scrollTo(hasText("Explicit Bot targets (typing @name does not run a Bot)"))
                compose.onAllNodes(isToggleable()).onLast().performScrollTo().assertIsDisplayed()
                    .performSemanticsAction(SemanticsActions.OnClick) { click -> assertTrue(click()) }.assertIsOn()
                textField("Message").performTextInput("Android explicit target acceptance")
                capture("03-explicit-bot-target")
                clickControl("Send")
                waitFor { state().cache!!.outbox.isEmpty() && state().cache!!.threads[group.id]?.messages?.any { it.text == "Android explicit target acceptance" } == true }
                assertEquals(group.botKeys, state().cache!!.threads[group.id]!!.messages.first { it.text == "Android explicit target acceptance" }.targets)
                val result = JSONObject().put("scope", "actual Teams Compose/Firebase feature harness; not full signed product")
                    .put("uid", uid).put("project", "demo-lingxi-teams-t0").put("authSdk", true).put("firestoreSdkServerRead", true)
                    .put("questionId", botQuestion.id).put("botDmId", dm.id).put("groupId", group.id)
                    .put("plainMentionTargets", 0).put("explicitTargets", group.botKeys.size)
                File(requireNotNull(activity.getExternalFilesDir("acceptance")), "result.json").writeText(result.toString(2))
            } catch (failure: Throwable) {
                try { recordFailure(failure) } catch (evidenceFailure: Throwable) { failure.addSuppressed(evidenceFailure) }
                throw failure
            }
        }
    }
}
