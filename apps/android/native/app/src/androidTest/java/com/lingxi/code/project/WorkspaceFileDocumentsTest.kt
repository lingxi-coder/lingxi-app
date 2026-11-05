package com.lingxi.code.project

import android.content.Intent
import android.content.pm.PackageManager
import android.os.Process
import android.provider.DocumentsContract
import androidx.documentfile.provider.DocumentFile
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class WorkspaceFileDocumentsTest {
    @Test fun recoveryPreservesRuntimeBrandSchedulerArtifacts() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val authority = "${instrumentation.context.packageName}.filesfixtures"
        val tree = DocumentsContract.buildTreeDocumentUri(authority, "root")
        instrumentation.context.startActivity(Intent(instrumentation.context, WorkspaceTestDocumentsActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        withTimeout(5_000) {
            while (context.checkUriPermission(tree, Process.myPid(), Process.myUid(),
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION) != PackageManager.PERMISSION_GRANTED) {
                delay(25)
            }
        }
        val root = requireNotNull(DocumentFile.fromTreeUri(context, tree))
        val fixture = requireNotNull(root.createDirectory(UUID.randomUUID().toString()))
        val native = File(context.cacheDir, "scheduler-recovery-${UUID.randomUUID()}").apply { mkdir() }
        try {
            val brand = requireNotNull(fixture.createDirectory(".atlas"))
            val nested = requireNotNull(requireNotNull(fixture.createDirectory("project")).createDirectory(".atlas"))
            val suffix = UUID.randomUUID().toString()
            val ownerBackup = "scheduled_tasks.lock.__lingxi_backup__$suffix"
            val ledgerBackup = "scheduled_owner_reservations.jsonl.__lingxi_backup__$suffix"
            val ownerTemp = "scheduled_tasks.lock.__lingxi_temp__$suffix"
            val nestedBackup = "scheduled_tasks.lock.__lingxi_backup__$suffix"
            for ((directory, name) in listOf(
                brand to "scheduled_tasks.lock", brand to ownerBackup, brand to ledgerBackup,
                brand to ownerTemp, brand to "notes.txt.__lingxi_backup__$suffix", nested to nestedBackup,
            )) requireNotNull(directory.createFile("application/octet-stream", name))
            val fixtureTree = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(fixture.uri))
            val repository = ProjectRepository(native, userHome = native,
                projectsGuestRoot = "/home/test/projects", userHomeGuestPath = "/home/test")
            SafProjectSynchronizer(context, repository, ".atlas").recoverExternalTransactions(fixtureTree)

            assertNotNull(brand.findFile("scheduled_tasks.lock"))
            assertNotNull(brand.findFile(ownerBackup))
            assertNotNull(brand.findFile(ledgerBackup))
            assertNotNull(brand.findFile(ownerTemp))
            assertNull(brand.findFile("scheduled_owner_reservations.jsonl"))
            assertNotNull(brand.findFile("notes.txt"))
            assertNotNull(nested.findFile("scheduled_tasks.lock"))
            assertNull(nested.findFile(nestedBackup))
        } finally { fixture.delete(); native.deleteRecursively() }
    }

    @Test fun importsAndExportsScopedDocumentTreesWithoutOverwriting() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val authority = "${instrumentation.context.packageName}.filesfixtures"
        val tree = DocumentsContract.buildTreeDocumentUri(authority, "root")
        // The test APK grants its own tree without acquiring another UiAutomation connection.
        instrumentation.context.startActivity(Intent(instrumentation.context, WorkspaceTestDocumentsActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        withTimeout(5_000) {
            while (context.checkUriPermission(tree, Process.myPid(), Process.myUid(),
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION) != PackageManager.PERMISSION_GRANTED) {
                delay(25)
            }
        }
        val root = requireNotNull(DocumentFile.fromTreeUri(context, tree))
        val fixture = requireNotNull(root.createDirectory(UUID.randomUUID().toString()))
        val native = File(context.cacheDir, "document-test-${UUID.randomUUID()}").apply { mkdir() }
        try {
            val source = requireNotNull(fixture.createDirectory("source"))
            val file = requireNotNull(source.createFile("text/plain", "note.txt"))
            context.contentResolver.openOutputStream(file.uri)!!.use { it.write("system document".toByteArray()) }
            val incoming = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(source.uri))
            val repo = WorkspaceDirectoryRepository(native, "/home/test", "/home/test/work", "/home/test/work/projects", File(native, ".config"), listOf("/home/test/work", "/home/test/work/projects"))
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
        } finally { fixture.delete(); native.deleteRecursively() }
    }
}
