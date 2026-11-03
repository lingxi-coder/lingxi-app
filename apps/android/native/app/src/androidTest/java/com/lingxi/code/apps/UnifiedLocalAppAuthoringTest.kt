package com.lingxi.code.apps

import android.content.Intent
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.bindings.android.AndroidMediaDrainStatus
import com.lingxi.code.bindings.runtime.waitForMobileSessionRelease
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.EngineConversationSource
import com.lingxi.code.conversation.SessionActivationKind
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.AuthoringSourceIdentity
import com.lingxi.code.project.MobileUserStorage
import com.lingxi.code.project.PreparedAuthoringGrant
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.project.ScopeStateStore
import com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Real engine, native source proof and owner receipt; no model-backed tool call is simulated. */
@RunWith(AndroidJUnit4::class)
class UnifiedLocalAppAuthoringTest {
    @Test fun pendingSourceRecoversExactOwnerAndRejectsOwnerlessPublication() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val apps = UnifiedLocalApps.get(context)
        val token = UUID.randomUUID().toString()
        val projectId = UUID.randomUUID().toString()
        val selectionId = UUID.randomUUID().toString()
        val workspaceId = UUID.randomUUID().toString()
        val projectStoreHolder = ViewModelStore()
        val chatStore = ViewModelStore()
        NativeAppActivityHost(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).use {
            val projectStore = withContext(Dispatchers.Main.immediate) {
                ViewModelProvider(projectStoreHolder, ProjectStore.factory(context))[ProjectStore::class.java]
            }
            withTimeout(30_000) { projectStore.state.first { !it.loading && !it.storageUnavailable } }
            val intents = AuthoringIntentStore(context)
            val request = AuthoringRequest(selectionId, "create", "Authoring $token", projectId, workspaceId)
            intents.saveRequest(request, SessionMode.Code)
            projectStore.createAuthoringPlaceholder(request.name, SessionMode.Code, projectId, selectionId)
            val prepared = apps.createAuthoringDirectory(request.name, selectionId, SessionMode.Code)
            val sourceDirectory = prepared.path
            val sourceFile = apps.hostFile(sourceDirectory)
            sourceFile.listFiles().orEmpty().forEach { check(it.deleteRecursively()) }
            check(File(context.filesDir, "local-app-acceptance-fixtures/static").copyRecursively(sourceFile, overwrite = true))
            val selection = AuthoringIntent(selectionId, prepared.sourceId, prepared.requestId, sourceDirectory, projectId, workspaceId, null)
            intents.savePrepared(selection, SessionMode.Code)
            val project = projectStore.registerAuthoringWorkspace(request.name, sourceDirectory, SessionMode.Code, projectId, selectionId)
            val identity = AuthoringSourceIdentity(null, projectId, sourceFile.absolutePath, sourceDirectory)
            val admission = checkNotNull(PreparedAuthoringGrant(identity, 1).consume(identity, 1, true))
            val first = EngineConversationSource.createInBackground(context, project.workspace, "project.$projectId", SessionMode.Code,
                reuseProcessSource = false, pendingAuthoringAdmission = admission) as EngineConversationSource
            val vm = withContext(Dispatchers.Main.immediate) { ChatViewModel(first).also { chatStore.put("authoring", it) } }
            val original = vm.awaitAppAuthoringSession(sourceDirectory)
            val sessionId = canonicalSessionId(original.sessionId)
            assertFalse(first.turnAdmissionReady)
            intents.attach(selectionId, sessionId, original.transcriptPath, SessionMode.Code)
            assertTrue(ScopeStateStore(context).claimSessionOwner(sessionId, "project.$projectId", SessionMode.Code))
            assertTrue(projectStore.recordStartedSession(projectId, sessionId, request.name, SessionMode.Code,
                transcriptPath = original.transcriptPath, markActive = { false }))
            intents.markCatalogued(sessionId)
            val receipt = apps.getOrRecoverAuthoringGrant(sourceDirectory, projectId, sessionId, SessionMode.Code)
            assertEquals(workspaceId, receipt.workspaceId)
            assertEquals(AppProjectRef("mobileProject", projectId = projectId), receipt.projectRef)
            assertEquals(sessionId, receipt.authoringSessionId)
            assertTrue(first.acknowledgeAuthoringBinding(identity, sessionId))
            assertTrue(first.turnAdmissionReady)
            assertTrue(runCatching { first.newSession() }.isFailure)
            assertTrue(runCatching { first.resumeSession(UUID.randomUUID().toString()) }.isFailure)
            val publication = runCatching {
                apps.authoringRequest(com.lingxi.code.model.SessionMode.Code, "publish") { put("sourcePath", sourceFile.absolutePath); put("expectedRevision", 0) }
            }.exceptionOrNull()
            assertTrue(publication is AppServiceException)
            assertEquals("invalidOperation", (publication as AppServiceException).code)
            withContext(Dispatchers.Main.immediate) { chatStore.clear() }
            val paths = MobileUserStorage.directories(context)
            assertTrue(waitForMobileSessionRelease(paths.configHome, sessionId, 8_000u))
            assertEquals(AndroidMediaDrainStatus.READY, AndroidNativeMediaHostProvider.get(context).waitForAttachmentDrain(8_000u))
            val reopened = EngineConversationSource.createInBackground(context, project.workspace, "project.$projectId", SessionMode.Code,
                reuseProcessSource = false, expectedSessionId = sessionId, initialSessionTranscriptPath = original.transcriptPath)
                as EngineConversationSource
            try {
                val activated = withTimeout(30_000) { reopened.activeSessionState.filterNotNull().first() }
                assertEquals(sessionId, canonicalSessionId(activated.sessionId))
                assertEquals(SessionActivationKind.Resumed, activated.kind)
                assertTrue(reopened.turnAdmissionReady)
            } finally {
                reopened.close()
                withContext(Dispatchers.Main.immediate) { projectStoreHolder.clear() }
            }
        }
    }
}
