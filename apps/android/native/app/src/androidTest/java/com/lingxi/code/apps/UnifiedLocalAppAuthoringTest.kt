package com.lingxi.code.apps

import android.content.Intent
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.EngineConversationSource
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionMode
import com.lingxi.code.project.ProjectWorkspace
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.URL
import java.util.UUID

/** Actual ordinary engine sessions; no provider inference or special App engine. */
@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppAuthoringTest {
    @Test fun registeredFirstPublishPersistsBindingAndMovedSessionRecoversAtSameSource() = runBlocking<Unit> {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val token = UUID.randomUUID().toString()
        val projectId = UUID.randomUUID().toString()
        val directory = apps.createAuthoring("Authoring $token")
        val workspaceId = UUID.randomUUID().toString()
        val projectRef = AppProjectRef("mobileProject", projectId = projectId)
        val source = apps.hostFile(directory)
        val movedDirectory = "$directory/moved"
        source.deleteRecursively()
        check(File(context.filesDir, "local-app-acceptance-fixtures/static").copyRecursively(source))
        File(source, "moved").mkdirs()
        val store = ViewModelStore()
        val transcriptPaths = mutableSetOf<String>()
        var record: AppRecord? = null
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            try {
                val workspace = ProjectWorkspace(projectId, source.absolutePath, directory)
                val first = EngineConversationSource.createInBackground(context, workspace, "project.$projectId", SessionMode.Code, reuseProcessSource = false)
                lateinit var vm: ChatViewModel
                withContext(Dispatchers.Main.immediate) { vm = ChatViewModel(first); store.put("authoring", vm); vm.startNewSession() }
                val original = vm.awaitAppAuthoringSession(directory)
                transcriptPaths.add(original.transcriptPath)
                val pending = apps.registerAuthoring(JSONObject()
                    .put("workspaceId", workspaceId)
                    .put("sourceDirectory", directory)
                    .put("projectRef", projectRef.toJson())
                    .put("authoringSessionId", canonicalSessionId(original.sessionId)))
                assertNull(pending.appId)
                assertEquals(directory, pending.sourceDirectory)
                assertEquals(workspaceId, pending.workspaceId)
                assertEquals(projectRef, pending.projectRef)
                assertEquals(canonicalSessionId(original.sessionId), pending.authoringSessionId)

                val published = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
                val createdRecord = published.getJSONObject("record").appRecord()
                record = createdRecord
                val publishedBinding = createdRecord.authoring!!
                assertEquals(createdRecord.id, publishedBinding.appId)
                assertEquals(directory, publishedBinding.sourceDirectory)
                assertEquals(workspaceId, publishedBinding.workspaceId)
                assertEquals(projectRef, publishedBinding.projectRef)
                assertEquals(canonicalSessionId(original.sessionId), publishedBinding.authoringSessionId)
                val originalBinding = AppAuthoringSessionContext(workspaceId, directory, projectRef, "code")
                val resumed = apps.prepareAuthoring(createdRecord.id, mapOf(canonicalSessionId(original.sessionId) to originalBinding))
                assertEquals("resumeSession", resumed.action)
                assertEquals(canonicalSessionId(original.sessionId), resumed.authoring?.authoringSessionId)

                val launch = apps.start(createdRecord.id)
                val layout = apps.request("layout", createdRecord.id) as JSONObject
                val data = File(layout.getString("dataPath"), "authoring-sentinel").apply { parentFile!!.mkdirs(); writeText("keep-data") }
                suspend fun bind(sessionId: String) {
                    apps.request("bindAuthoring", createdRecord.id) {
                        put("authoring", JSONObject().put("workspaceId", workspaceId).put("sourceDirectory", directory)
                            .put("projectRef", projectRef.toJson()).put("authoringSessionId", canonicalSessionId(sessionId)).put("appId", createdRecord.id))
                    }
                }
                bind(original.sessionId)
                first.changeWorkspace(movedDirectory, original.sessionId)
                val moved = withTimeout(20_000) { first.workspaceState.first { it?.cwd == movedDirectory }!! }
                assertEquals(original.sessionId, moved.sessionId)
                assertEquals(directory, apps.get(createdRecord.id).authoring!!.sourceDirectory)
                val movedBinding = AppAuthoringSessionContext(workspaceId, moved.cwd, projectRef, "code")
                assertEquals("createSession", apps.prepareAuthoring(createdRecord.id, mapOf(moved.sessionId to movedBinding)).action)
                assertTrue(withContext(Dispatchers.Main.immediate) { vm.switchWorkspaceSource(projectId = projectId, newSessionDirectory = directory, createSource = {
                    EngineConversationSource.createInBackground(context, workspace, "project.$projectId", SessionMode.Code, reuseProcessSource = false)
                }) })
                val recovered = vm.awaitAppAuthoringSession(directory)
                transcriptPaths.add(recovered.transcriptPath)
                assertNotEquals(original.sessionId, recovered.sessionId)
                bind(recovered.sessionId)
                val recoveredBinding = AppAuthoringSessionContext(workspaceId, directory, projectRef, "code")
                val again = apps.prepareAuthoring(createdRecord.id, mapOf(recovered.sessionId to recoveredBinding))
                assertEquals("resumeSession", again.action)
                assertEquals(recovered.sessionId, again.authoring?.authoringSessionId)
                withContext(Dispatchers.Main.immediate) { vm.openSession(SessionRef(recovered.sessionId, "App source")) }
                assertEquals(recovered.sessionId, vm.awaitAppAuthoringSession(directory, recovered.sessionId).sessionId)
                assertEquals(launch.run.runId, (apps.request("status", createdRecord.id) as JSONObject).getJSONObject("run").getString("runId"))
                assertEquals("keep-data", data.readText())
                withContext(Dispatchers.IO) {
                    val page = URL(launch.url).openConnection().apply { connectTimeout = 5000; readTimeout = 5000 }
                    assertTrue(page.getInputStream().bufferedReader().use { it.readText() }.contains("html"))
                }
                android.util.Log.i("LocalAppAcceptance", "AUTHORING_RECOVERY PASS real-engine cd-keeps-UUID fixed-app-source new-session-explicit-cwd saved-reference second-open-reuses-UUID running-app-and-data-independent")
            } finally {
                withContext(Dispatchers.Main.immediate) { store.clear() }
                record?.let { runCatching { apps.remove(it.id) } }
                for (path in transcriptPaths) {
                    // Remove only the exact transcript identified by our engine.
                    if (path.contains(token) || path.contains(projectId)) runCatching { apps.hostFile(path).delete() }
                }
                source.deleteRecursively()
            }
        }
    }
}
