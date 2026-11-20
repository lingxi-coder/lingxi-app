package com.lingxi.code.teams

import android.content.ContextWrapper
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextReplacement
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreSettings
import com.google.firebase.firestore.MemoryCacheSettings
import com.lingxi.code.R
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.TabShellState
import com.lingxi.code.testing.finishComposeFixture
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LingxiDataStagingUiTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Test fun ownerOpensTemplateAndWritesThroughTeams() {
        val args = InstrumentationRegistry.getArguments()
        val ports = listOf("lingxiDataAuth", "lingxiDataFirestore", "lingxiDataApi").map { args.getString(it)?.toIntOrNull() }
        assumeTrue(ports.all { it != null && it in 1024..65535 })
        val (authPort, firestorePort, apiPort) = ports.map { requireNotNull(it) }
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = File(target.cacheDir, "lingxi-data-ui-${UUID.randomUUID()}").apply { mkdirs() }
        val isolated = object : ContextWrapper(target) {
            override fun getCacheDir(): File = directory
            override fun getNoBackupFilesDir(): File = directory
        }
        val sdk = FirebaseApp.initializeApp(isolated, FirebaseOptions.Builder().setProjectId("demo-lingxi-data")
            .setApplicationId("1:123456789:android:0000000000000000").setApiKey("demo").build(), "data-ui-${UUID.randomUUID()}")
        val auth = FirebaseAuth.getInstance(sdk).apply { useEmulator("127.0.0.1", authPort) }
        val db = FirebaseFirestore.getInstance(sdk).apply {
            firestoreSettings = FirebaseFirestoreSettings.Builder().setHost("127.0.0.1:$firestorePort").setSslEnabled(false)
                .setLocalCacheSettings(MemoryCacheSettings.newBuilder().build()).build()
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        val prefs = TeamsPrefs(PreferenceDataStoreFactory.create(scope = scope) { File(directory, "teams.preferences_pb") })
        val workspace = TeamsWorkspace(isolated, scope, auth, db, prefs, "http://127.0.0.1:$apiPort")
        try {
            runBlocking {
                withTimeout(20_000) { auth.signInWithEmailAndPassword("owner@fixture.invalid", "AccountFixture123").await() }
            }
            ActivityScenario.launch(ComponentActivity::class.java).use { scenario ->
                lateinit var activity: ComponentActivity
                scenario.onActivity { screen ->
                    activity = screen
                    workspace.start()
                    screen.setContent {
                        LingXiTheme {
                            val tabs = remember { TabShellState(AppTab.Teams) }
                            CompositionLocalProvider(LocalTabHost provides tabs) {
                                TeamsTab(joinCode = null, onJoinCodeHandled = {}, workspaceOverride = workspace)
                            }
                        }
                    }
                }
                try {
                    checkRecordAndSave(activity, workspace)
                } finally {
                    finishComposeFixture(activity)
                }
            }
        } finally {
            runBlocking { withContext(Dispatchers.Main) { workspace.teamsModel.stop(); workspace.close() } }
            scope.cancel()
            auth.signOut()
            runBlocking { withTimeout(10_000) { db.terminate().await() } }
            sdk.delete()
            directory.deleteRecursively()
        }
    }

    private fun checkRecordAndSave(context: ComponentActivity, workspace: TeamsWorkspace) {
        val conversation = "orders-chat"
        val room = TeamsTags.row(conversation)
        compose.waitUntil(25_000) { compose.onAllNodes(hasTestTag(room)).fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithTag(room).performClick()
        val open = context.getString(R.string.teams_data_open)
        awaitText(open)
        compose.onNodeWithText(open).performClick()
        awaitText("order-summary")
        compose.onNodeWithText("order-summary").performClick()
        awaitText("Prepare shipment")

        val current = runBlocking {
            withTimeout(10_000) { withContext(Dispatchers.Main) {
                val record = requireNotNull(workspace.messaging.value.cache?.threads?.get(conversation)?.messages
                    ?.firstOrNull { it.id == "staging-message" }?.dataRef)
                val latest = findLatest(workspace, conversation, record.dataset, record.record)
                if (latest.revision == record.revision) {
                    workspace.messagingModel.data(conversation, JSONObject().put("action", "put").put("dataset", record.dataset)
                        .put("record", record.record).put("expectedRevision", record.revision).put("schemaVersion", record.schemaVersion)
                        .put("value", JSONObject().put("title", "Updated before Android saves")), UUID.randomUUID().toString())
                }
                record
            } }
        }
        val title = "Packed from Android ${UUID.randomUUID().toString().take(8)}"
        val draft = JSONObject().put("title", title).toString()
        val edit = context.getString(R.string.teams_message_edit)
        val save = context.getString(R.string.teams_data_save)
        val editor = hasSetTextAction() and hasText(context.getString(R.string.teams_data_json))
        fun editRecord() {
            awaitText(edit)
            compose.onNodeWithText(edit).performClick()
            compose.onNode(editor).performTextReplacement(draft)
            compose.onNodeWithText(save).performClick()
        }
        editRecord()
        val reload = context.getString(R.string.teams_data_latest)
        awaitText(reload)
        compose.onNodeWithText(reload).performClick()
        editRecord()
        awaitText("order-summary")
        compose.onNodeWithText("order-summary").performClick()
        awaitText(title)

        runBlocking {
            withTimeout(10_000) { withContext(Dispatchers.Main) {
                val latest = findLatest(workspace, conversation, current.dataset, current.record)
                val stored = workspace.messagingModel.data(conversation, JSONObject().put("action", "get").put("reference", latest.json()))
                assertEquals(title, stored.getJSONObject("record").getJSONObject("value").getString("title"))
            } }
        }
    }

    private suspend fun findLatest(workspace: TeamsWorkspace, conversation: String, dataset: String, record: String): TeamDataRef {
        val response = workspace.messagingModel.data(conversation, JSONObject().put("action", "latest").put("dataset", dataset).put("record", record))
        return requireNotNull(TeamDataRef.parse(response))
    }

    private fun awaitText(text: String) {
        compose.waitUntil(20_000) { compose.onAllNodes(hasText(text)).fetchSemanticsNodes().isNotEmpty() }
    }
}
