package com.lingxi.code.teams

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreSettings
import com.google.firebase.firestore.MemoryCacheSettings
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LingxiDataStagingTest {
    @Test fun opensTeamRecordAndItsTemplateWithAnIsolatedAccount() {
        val args = InstrumentationRegistry.getArguments()
        val ports = listOf("lingxiDataAuth", "lingxiDataFirestore", "lingxiDataApi").map { args.getString(it)?.toIntOrNull() }
        assumeTrue(ports.all { it != null && it in 1024..65535 })
        val (authPort, firestorePort, apiPort) = ports.map { requireNotNull(it) }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val sdk = FirebaseApp.initializeApp(context, FirebaseOptions.Builder().setProjectId("demo-lingxi-data")
            .setApplicationId("1:123456789:android:0000000000000000").setApiKey("demo").build(), "data-staging-${UUID.randomUUID()}")
        val auth = FirebaseAuth.getInstance(sdk).apply { useEmulator("127.0.0.1", authPort) }
        val db = FirebaseFirestore.getInstance(sdk).apply {
            firestoreSettings = FirebaseFirestoreSettings.Builder().setHost("127.0.0.1:$firestorePort").setSslEnabled(false)
                .setLocalCacheSettings(MemoryCacheSettings.newBuilder().build()).build()
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        val path = File(context.cacheDir, "data-staging-${UUID.randomUUID()}")
        val model = TeamMessagingModel(scope, path, "http://127.0.0.1:$apiPort", auth, db)
        try {
            runBlocking {
                withTimeout(25_000) {
                    val account = auth.signInWithEmailAndPassword("reader@fixture.invalid", "AccountFixture123").await()
                    val uid = requireNotNull(account.user).uid
                    val team = TeamSummary("data-team", "Data Staging", 2, "member", 1)
                    val conversation = "orders-chat"
                    withContext(Dispatchers.Main) { model.start(uid, team); model.select(conversation) }
                    val state = model.state.first { snapshot ->
                        snapshot.cache?.threads?.get(conversation)?.messages?.any { it.id == "staging-message" } == true
                    }
                    val reference = requireNotNull(state.cache?.threads?.get(conversation)?.messages
                        ?.first { it.id == "staging-message" }?.dataRef)
                    withContext(Dispatchers.Main) {
                        val result = model.data(conversation, JSONObject().put("action", "get").put("reference", reference.json()))
                        assertEquals("Prepare shipment", result.getJSONObject("record").getJSONObject("value").getString("title"))
                        val views = model.data(conversation, JSONObject().put("action", "templateViews").put("dataset", reference.dataset))
                        assertEquals(listOf("order-summary"), views.getJSONArray("views").objects().map { it.getString("view") })
                        val denied = runCatching {
                            model.data(conversation, JSONObject().put("action", "put").put("dataset", reference.dataset)
                                .put("record", reference.record).put("expectedRevision", reference.revision).put("schemaVersion", reference.schemaVersion)
                                .put("value", JSONObject().put("title", "Unauthorized")), UUID.randomUUID().toString())
                        }.exceptionOrNull() as? TeamMessageFailure
                        assertEquals("DATA_ACCESS_REQUIRED", denied?.code)
                    }
                }
            }
        } finally {
            runBlocking { withContext(Dispatchers.Main) { model.stop() } }
            scope.cancel()
            auth.signOut()
            runBlocking { withTimeout(10_000) { db.terminate().await() } }
            sdk.delete()
            path.deleteRecursively()
        }
    }
}
