package com.lingxi.code.project

import android.content.Intent
import android.content.pm.PackageManager
import android.os.Process
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

@RunWith(AndroidJUnit4::class)
class WorkspaceFileDocumentsTest {
    @Test fun importsAndExportsScopedDocumentTreesWithoutOverwriting() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val testContext = instrumentation.context
        val context = instrumentation.targetContext
        val authority = WorkspaceTestDocumentsProvider.authority(testContext)
        assertEquals(context.packageName, WorkspaceTestDocumentsProvider.targetPackage(testContext))
        val automation = instrumentation.uiAutomation
        fun startTestActivity(revoke: Boolean): String {
            val action = if (revoke) "-a ${WorkspaceTestDocumentsActivity.ACTION_REVOKE} " else ""
            return automation.executeShellCommand("am start -W ${action}-n ${testContext.packageName}/com.lingxi.code.project.WorkspaceTestDocumentsActivity").use { descriptor ->
                java.io.FileInputStream(descriptor.fileDescriptor).use { it.readBytes().decodeToString() }
            }
        }
        val tree = DocumentsContract.buildTreeDocumentUri(authority, "root")
        try {
            // Start the test APK so its provider grants this variant's scoped tree to its target.
            val launch = startTestActivity(revoke = false)
            assertTrue(launch, launch.contains("Status: ok"))
            assertEquals(PackageManager.PERMISSION_GRANTED, context.checkUriPermission(tree, Process.myPid(), Process.myUid(),
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION))
            val root = requireNotNull(DocumentFile.fromTreeUri(context, tree))
            val fixture = requireNotNull(root.createDirectory(UUID.randomUUID().toString()))
            val native = File(context.cacheDir, "document-test-${UUID.randomUUID()}").apply { mkdir() }
            try {
                val source = requireNotNull(fixture.createDirectory("source"))
                val file = requireNotNull(source.createFile("text/plain", "note.txt"))
                context.contentResolver.openOutputStream(file.uri)!!.use { it.write("system document".toByteArray()) }
                val incoming = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(source.uri))
                val repo = WorkspaceDirectoryRepository(native, "/home/test", "/home/test/work", "/home/test/work/projects", File(native, ".config"))
                val imported = repo.importDocuments(context, listOf(incoming), "/home/test", true, FileConflict.KeepBoth) {}
                assertTrue(imported.toString(), imported.isEmpty())
                assertEquals("system document", File(native, "source/note.txt").readText())
                assertTrue(repo.importDocuments(context, listOf(file.uri), "/home/test", false, FileConflict.KeepBoth) {}.isEmpty())
                assertEquals("system document", File(native, "note.txt").readText())
                val outgoing = requireNotNull(fixture.createDirectory("export"))
                val destination = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(outgoing.uri))
                assertTrue(repo.exportDocuments(context, listOf("/home/test/source"), destination, FileConflict.KeepBoth) {}.isEmpty())
                val exported = requireNotNull(outgoing.findFile("source")?.findFile("note.txt"))
                assertEquals("system document", context.contentResolver.openInputStream(exported.uri)!!.bufferedReader().use { it.readText() })
                assertTrue(repo.exportDocuments(context, listOf("/home/test/source"), destination, FileConflict.KeepBoth) {}.isEmpty())
                assertNotNull(outgoing.findFile("source (2)"))
                assertTrue(outgoing.listFiles().none { it.name.orEmpty().startsWith(".lingxi-export-") })
            } finally {
                try { assertTrue("test document fixture was not removed", fixture.delete()) }
                finally { native.deleteRecursively() }
            }
        } finally {
            val cleanup = startTestActivity(revoke = true)
            assertTrue(cleanup, cleanup.contains("Status: ok"))
            assertEquals(PackageManager.PERMISSION_DENIED, context.checkUriPermission(tree, Process.myPid(), Process.myUid(),
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION))
        }
    }
}
