package com.lingxi.code.apps

import android.content.Intent
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.EngineConversationSource
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
    @Test fun movedSessionRecoversAtRecordedSourceAndReusesConfirmedUuid() = runBlocking<Unit> {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val token = UUID.randomUUID().toString()
        val projectId = UUID.randomUUID().toString()
        val directory = "/home/lingxi/.lingxi/.apps/authoring-$token"
        val source = apps.hostFile(directory)
        val movedDirectory = "$directory/moved"
        check(File(context.filesDir, "local-app-acceptance-fixtures/static").copyRecursively(source))
        File(source, "moved").mkdirs()
        val published = apps.request("publish") { put("sourcePath", source.absolutePath); put("expectedRevision", 0) } as JSONObject
        val record = published.getJSONObject("record").appRecord()
        val store = ViewModelStore()
        val transcriptPaths = mutableSetOf<String>()
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            try {
                val launch = apps.start(record.id)
                val layout = apps.request("layout", record.id) as JSONObject
                val data = File(layout.getString("dataPath"), "authoring-sentinel").apply { parentFile!!.mkdirs(); writeText("keep-data") }
                val workspace = ProjectWorkspace(projectId, source.absolutePath, directory)
                val first = EngineConversationSource.createInBackground(context, workspace, "project.$projectId", SessionMode.Code, reuseProcessSource = false)
                lateinit var vm: ChatViewModel
                withContext(Dispatchers.Main.immediate) { vm = ChatViewModel(first); store.put("authoring", vm); vm.startNewSession() }
                val original = vm.awaitAppAuthoringSession(directory)
                transcriptPaths.add(original.transcriptPath)
                suspend fun bind(sessionId: String) {
                    apps.request("bindAuthoring", record.id) { put("authoring", JSONObject().put("sourceDirectory", directory).put("sourceSessionId", sessionId).put("projectId", projectId)) }
                }
                bind(original.sessionId)
                first.changeWorkspace(movedDirectory, original.sessionId)
                val moved = withTimeout(20_000) { first.workspaceState.first { it?.cwd == movedDirectory }!! }
                assertEquals(original.sessionId, moved.sessionId)
                assertEquals(directory, apps.get(record.id).authoring!!.sourceDirectory)
                assertEquals("createSession", apps.prepareAuthoring(record.id, mapOf(moved.sessionId to moved.cwd)).action)
                assertTrue(withContext(Dispatchers.Main.immediate) { vm.switchWorkspaceSource(projectId = projectId, newSessionDirectory = directory, createSource = {
                    EngineConversationSource.createInBackground(context, workspace, "project.$projectId", SessionMode.Code, reuseProcessSource = false)
                }) })
                val recovered = vm.awaitAppAuthoringSession(directory)
                transcriptPaths.add(recovered.transcriptPath)
                assertNotEquals(original.sessionId, recovered.sessionId)
                bind(recovered.sessionId)
                val again = apps.prepareAuthoring(record.id, mapOf(recovered.sessionId to recovered.cwd))
                assertEquals("resumeSession", again.action)
                assertEquals(recovered.sessionId, again.sessionId)
                withContext(Dispatchers.Main.immediate) { vm.openSession(SessionRef(recovered.sessionId, "App source")) }
                assertEquals(recovered.sessionId, vm.awaitAppAuthoringSession(directory, recovered.sessionId).sessionId)
                assertEquals(launch.run.runId, (apps.request("status", record.id) as JSONObject).getJSONObject("run").getString("runId"))
                assertEquals("keep-data", data.readText())
                withContext(Dispatchers.IO) {
                    val page = URL(launch.url).openConnection().apply { connectTimeout = 5000; readTimeout = 5000 }
                    assertTrue(page.getInputStream().bufferedReader().use { it.readText() }.contains("html"))
                }
                android.util.Log.i("LocalAppAcceptance", "AUTHORING_RECOVERY PASS real-engine cd-keeps-UUID fixed-app-source new-session-explicit-cwd saved-reference second-open-reuses-UUID running-app-and-data-independent")
            } finally {
                withContext(Dispatchers.Main.immediate) { store.clear() }
                apps.remove(record.id)
                for (path in transcriptPaths) {
                    // Remove only the exact transcript identified by our engine.
                    if (path.contains(token) || path.contains(projectId)) runCatching { apps.hostFile(path).delete() }
                }
                source.deleteRecursively()
            }
        }
    }
}
