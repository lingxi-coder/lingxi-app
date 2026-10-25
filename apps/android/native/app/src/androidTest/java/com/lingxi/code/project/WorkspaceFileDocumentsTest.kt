package com.lingxi.code.project

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
        val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
        // Start the test APK so its provider can grant a scoped test-tree URI to the target APK.
        automation.executeShellCommand("am start -W -n com.lingxi.code.debug.test/com.lingxi.code.project.WorkspaceTestDocumentsActivity").use { descriptor ->
            java.io.FileInputStream(descriptor.fileDescriptor).use { it.readBytes() }
        }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val tree = DocumentsContract.buildTreeDocumentUri("com.lingxi.code.debug.test.filesfixtures", "root")
        val root = requireNotNull(DocumentFile.fromTreeUri(context, tree))
        val fixture = requireNotNull(root.createDirectory(UUID.randomUUID().toString()))
        val native = File(context.cacheDir, "document-test-${UUID.randomUUID()}").apply { mkdir() }
        try {
            val source = requireNotNull(fixture.createDirectory("source"))
            val file = requireNotNull(source.createFile("text/plain", "note.txt"))
            context.contentResolver.openOutputStream(file.uri)!!.use { it.write("system document".toByteArray()) }
            val incoming = DocumentsContract.buildTreeDocumentUri("com.lingxi.code.debug.test.filesfixtures", DocumentsContract.getDocumentId(source.uri))
            val repo = WorkspaceDirectoryRepository(native, "/home/test", "/home/test/work", "/home/test/work/projects", File(native, ".config"))
            val imported = repo.importDocuments(context, listOf(incoming), "/home/test", true, FileConflict.KeepBoth) {}
            assertTrue(imported.toString(), imported.isEmpty())
            assertEquals("system document", File(native, "source/note.txt").readText())
            assertTrue(repo.importDocuments(context, listOf(file.uri), "/home/test", false, FileConflict.KeepBoth) {}.isEmpty())
            assertEquals("system document", File(native, "note.txt").readText())
            val outgoing = requireNotNull(fixture.createDirectory("export"))
            val destination = DocumentsContract.buildTreeDocumentUri("com.lingxi.code.debug.test.filesfixtures", DocumentsContract.getDocumentId(outgoing.uri))
            assertTrue(repo.exportDocuments(context, listOf("/home/test/source"), destination, FileConflict.KeepBoth) {}.isEmpty())
            val exported = requireNotNull(outgoing.findFile("source")?.findFile("note.txt"))
            assertEquals("system document", context.contentResolver.openInputStream(exported.uri)!!.bufferedReader().use { it.readText() })
            assertTrue(repo.exportDocuments(context, listOf("/home/test/source"), destination, FileConflict.KeepBoth) {}.isEmpty())
            assertNotNull(outgoing.findFile("source (2)"))
            assertTrue(outgoing.listFiles().none { it.name.orEmpty().startsWith(".lingxi-export-") })
        } finally { fixture.delete(); native.deleteRecursively() }
    }
}
