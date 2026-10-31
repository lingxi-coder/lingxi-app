package com.lingxi.code.teams

import android.app.Application
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Runs only against the explicit local Teams cache acceptance fixture. */
class TeamFirestoreCacheTest {
    @Test fun sdkCacheReopensOfflineAndStreamsOldChangesReadMarksAndRevocation() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        assumeTrue("Explicit local emulator acceptance only", InstrumentationRegistry.getArguments().getString("teamsCacheEmulator") == "true")
        val application = instrumentation.targetContext.applicationContext as Application
        val base = "http://127.0.0.1:41791"
        val fixture = TeamHttp.request("$base/fixture?client=android", "emulator-only", null)
        val options = FirebaseOptions.Builder().setProjectId("demo-lingxi-teams-t0").setApiKey("AIzaSyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").setApplicationId("1:123456789:android:0123456789abcdef").build()
        val name = "teams-cache-acceptance"
        val phase = InstrumentationRegistry.getArguments().getString("teamsCachePhase")
        require(phase == "warm" || phase == "offline") { "Explicit warm/offline phase required" }
        val app = FirebaseApp.initializeApp(application, options, name)
        val auth = FirebaseAuth.getInstance(app)
        val db = FirebaseFirestore.getInstance(app)
        var model: TeamMessagingModel? = null
        val uid = fixture.getString("uid")
        val teamId = fixture.getString("teamId")
        val id = fixture.getString("conversationId")
        val team = TeamSummary(teamId, "SDK cache fixture", 1, "owner", 1)
        suspend fun until(predicate: () -> Boolean) { withTimeout(20000) { while (!predicate()) delay(20) } }
        fun start() { instrumentation.runOnMainSync { model = TeamMessagingModel(application, base, auth, db).also { it.start(uid, team); it.select(id) } } }
        try {
            auth.useEmulator("127.0.0.1", 9098); db.useEmulator("127.0.0.1", 8085)
            if (phase == "warm") {
                auth.signInWithCustomToken(fixture.getString("customToken")).await()
                start(); val current = requireNotNull(model)
                until {
                    val state = current.state.value
                    state.cache?.threads?.get(id)?.messages?.size == 125 && state.cache.threads[id]?.messages?.firstOrNull()?.text == "Body 1" && state.cache.conversations.firstOrNull()?.lastReadSeq == 0L && !state.offline
                }
                println("TEAMS_CACHE warm-server-snapshot 125")
                return@runBlocking
            }
            assertEquals(uid, auth.currentUser?.uid)
            db.disableNetwork().await()
            start(); val current = requireNotNull(model)
            until { current.state.value.cache?.threads?.get(id)?.messages?.size == 125 }
            assertTrue(current.state.value.offline)
            println("TEAMS_CACHE disk-cache-offline-process-restart 125")
            TeamHttp.request("$base/edit?client=android", "emulator-only", null)
            assertEquals("Body 1", current.state.value.cache!!.threads.getValue(id).messages.first().text)
            db.enableNetwork().await(); until { current.state.value.cache?.threads?.get(id)?.messages?.firstOrNull()?.text == "Edited oldest" }
            val messages = current.state.value.cache!!.threads.getValue(id).messages
            assertEquals("", messages[1].text); assertTrue(messages[1].recalled)
            TeamHttp.request("$base/read?client=android", "emulator-only", null)
            until { current.state.value.cache?.conversations?.firstOrNull()?.lastReadSeq == 120L }
            TeamHttp.request("$base/revoke?client=android", "emulator-only", null)
            until { current.state.value.revoked }; assertNull(current.state.value.cache)
            println("TEAMS_CACHE old-edit-recall-read-revocation PASS")
        } finally { instrumentation.runOnMainSync { model?.stop() }; db.terminate().await(); app.delete() }
    }
}
