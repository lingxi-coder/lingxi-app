package com.lingxi.code.project

import android.content.Intent
import android.content.Context
import android.content.BroadcastReceiver
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.app.Instrumentation
import android.net.Uri
import android.os.Build
import android.os.Process
import android.util.Log
import android.provider.DocumentsContract
import androidx.documentfile.provider.DocumentFile
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

@RunWith(AndroidJUnit4::class)
class WorkspaceFileDocumentsTest {
    private fun startTestActivity(instrumentation: Instrumentation, testContext: Context, id: String, revoke: Boolean) {
        val context = instrumentation.targetContext
        val request = UUID.randomUUID().toString()
        val latch = CountDownLatch(1)
        val response = AtomicReference<Intent?>()
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (intent.getStringExtra(WorkspaceTestDocumentsActivity.EXTRA_REQUEST_ID) == request && response.compareAndSet(null, intent)) {
                    latch.countDown()
                }
            }
        }
        val filter = IntentFilter(WorkspaceTestDocumentsActivity.ACTION_RESULT)
        if (Build.VERSION.SDK_INT >= 33) context.registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED)
        else @Suppress("DEPRECATION") context.registerReceiver(receiver, filter)
        val action = if (revoke) "-a ${WorkspaceTestDocumentsActivity.ACTION_REVOKE} " else ""
        val extras = "--es ${WorkspaceTestDocumentsActivity.EXTRA_FIXTURE_ID} $id --es ${WorkspaceTestDocumentsActivity.EXTRA_REQUEST_ID} $request "
        try {
            // Wait for the fixture receipt rather than Activity visibility or shell EOF.
            Log.i("LingxiTest", "Fixture before UiAutomation: $request")
            val automation = instrumentation.uiAutomation
            Log.i("LingxiTest", "Fixture before shell start: $request")
            // Extras do not distinguish Android intents for task reuse. Give
            // each request its own data URI so consecutive launches are delivered.
            automation.executeShellCommand("am start ${action}-d lingxi-fixture://request/$request ${extras}-n ${testContext.packageName}/com.lingxi.code.project.WorkspaceTestDocumentsActivity").use {
                Log.i("LingxiTest", "Fixture awaiting receipt: $request")
                assertTrue("test fixture action timed out (revoke=$revoke)", latch.await(20, TimeUnit.SECONDS))
            }
            Log.i("LingxiTest", "Fixture received receipt: $request")
            val result = requireNotNull(response.get())
            assertTrue(result.getStringExtra(WorkspaceTestDocumentsActivity.EXTRA_ERROR),
                result.getBooleanExtra(WorkspaceTestDocumentsActivity.EXTRA_SUCCESS, false))
        } finally {
            context.unregisterReceiver(receiver)
        }
    }

    private fun assertPermission(context: Context, tree: Uri, expected: Int) {
        for (mode in listOf(Intent.FLAG_GRANT_READ_URI_PERMISSION, Intent.FLAG_GRANT_WRITE_URI_PERMISSION)) {
            assertEquals(tree.toString(), expected, context.checkUriPermission(tree, Process.myPid(), Process.myUid(), mode))
        }
    }

    @Test fun importsAndExportsScopedDocumentTreesWithoutOverwriting() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val testContext = instrumentation.context
        val context = instrumentation.targetContext
        val authority = WorkspaceTestDocumentsProvider.authority(testContext)
        assertEquals(context.packageName, WorkspaceTestDocumentsProvider.targetPackage(testContext))
        // A broad grant from an older fixture would invalidate per-tree denial checks.
        assertPermission(context, DocumentsContract.buildTreeDocumentUri(authority, "root"), PackageManager.PERMISSION_DENIED)
        val fixtureId = UUID.randomUUID().toString()
        val tree = DocumentsContract.buildTreeDocumentUri(authority, fixtureId)
        val native = File(context.cacheDir, "document-test-${UUID.randomUUID()}").apply { mkdir() }
        var cleanupFixture: DocumentFile? = null
        val childTrees = mutableListOf<Uri>()
        try {
            // Start the test APK so its provider grants this variant's scoped tree to its target.
            startTestActivity(instrumentation, testContext, fixtureId, revoke = false)
            assertPermission(context, tree, PackageManager.PERMISSION_GRANTED)
            val fixture = requireNotNull(DocumentFile.fromTreeUri(context, tree))
            cleanupFixture = fixture
            val source = requireNotNull(fixture.createDirectory("source"))
            val file = requireNotNull(source.createFile("text/plain", "note.txt"))
            context.contentResolver.openOutputStream(file.uri)!!.use { it.write("system document".toByteArray()) }
            val incoming = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(source.uri))
            childTrees += incoming
            val repo = WorkspaceDirectoryRepository(native, "/home/test", "/home/test/work", "/home/test/work/projects", File(native, ".config"))
            val imported = repo.importDocuments(context, listOf(incoming), "/home/test", true, FileConflict.KeepBoth) {}
            assertTrue(imported.toString(), imported.isEmpty())
            assertEquals("system document", File(native, "source/note.txt").readText())
            assertTrue(repo.importDocuments(context, listOf(file.uri), "/home/test", false, FileConflict.KeepBoth) {}.isEmpty())
            assertEquals("system document", File(native, "note.txt").readText())
            val outgoing = requireNotNull(fixture.createDirectory("export"))
            val destination = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(outgoing.uri))
            childTrees += destination
            assertTrue(repo.exportDocuments(context, listOf("/home/test/source"), destination, FileConflict.KeepBoth) {}.isEmpty())
            val exported = requireNotNull(outgoing.findFile("source")?.findFile("note.txt"))
            assertEquals("system document", context.contentResolver.openInputStream(exported.uri)!!.bufferedReader().use { it.readText() })
            assertTrue(repo.exportDocuments(context, listOf("/home/test/source"), destination, FileConflict.KeepBoth) {}.isEmpty())
            assertNotNull(outgoing.findFile("source (2)"))
            assertTrue(outgoing.listFiles().none { it.name.orEmpty().startsWith(".lingxi-export-") })
        } finally {
            try { cleanupFixture?.let { assertTrue("test document fixture was not removed", it.delete()) } }
            finally {
                native.deleteRecursively()
                startTestActivity(instrumentation, testContext, fixtureId, revoke = true)
                assertPermission(context, tree, PackageManager.PERMISSION_DENIED)
                childTrees.forEach { assertPermission(context, it, PackageManager.PERMISSION_DENIED) }
            }
        }
    }

    @Test fun removingOneFixtureKeepsOtherFixturePermission() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val testContext = instrumentation.context
        val context = instrumentation.targetContext
        val authority = WorkspaceTestDocumentsProvider.authority(testContext)
        assertPermission(context, DocumentsContract.buildTreeDocumentUri(authority, "root"), PackageManager.PERMISSION_DENIED)
        val firstId = UUID.randomUUID().toString()
        val secondId = UUID.randomUUID().toString()
        val first = DocumentsContract.buildTreeDocumentUri(authority, firstId)
        val second = DocumentsContract.buildTreeDocumentUri(authority, secondId)
        try {
            for (id in listOf(firstId, secondId)) {
                startTestActivity(instrumentation, testContext, id, revoke = false)
            }
            assertPermission(context, first, PackageManager.PERMISSION_GRANTED)
            assertPermission(context, second, PackageManager.PERMISSION_GRANTED)
            startTestActivity(instrumentation, testContext, firstId, revoke = true)
            assertPermission(context, first, PackageManager.PERMISSION_DENIED)
            assertPermission(context, second, PackageManager.PERMISSION_GRANTED)
            assertTrue(requireNotNull(DocumentFile.fromTreeUri(context, second)).exists())
        } finally {
            try { startTestActivity(instrumentation, testContext, firstId, revoke = true) }
            finally { startTestActivity(instrumentation, testContext, secondId, revoke = true) }
        }
    }
}
