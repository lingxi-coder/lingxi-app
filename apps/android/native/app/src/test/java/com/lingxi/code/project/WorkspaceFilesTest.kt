package com.lingxi.code.project

import com.lingxi.code.R
import kotlinx.coroutines.*
import org.junit.*
import org.junit.Assert.*
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files

class WorkspaceFilesTest {
    @get:Rule val temporary = TemporaryFolder()
    private fun repo() = WorkspaceDirectoryRepository(temporary.root, "/home/brand", "/home/brand/work", "/home/brand/work/projects", File(temporary.root, ".config"))
    private fun write(path: String, value: String) = File(temporary.root, path).apply { parentFile!!.mkdirs(); writeText(value) }
    @Test fun recursiveSearchSkipsLinksAndSortsNaturally() = runBlocking {
        val repo = repo()
        write("file10.txt", "ten"); write("file2.txt", "two"); write(".hidden", "secret"); write("sub/needle.txt", "find")
        Files.createSymbolicLink(File(temporary.root, "loop").toPath(), temporary.root.toPath())
        val result = repo.files("/home/brand", false, "needle", true)
        assertEquals(listOf("/home/brand/sub/needle.txt"), result.map { it.path })
        val sorted = sortedFiles(repo.files("/home/brand", false, "", false), FileSort.Name)
        assertEquals(listOf("file2.txt", "file10.txt"), sorted.filter { !it.directory }.map { it.name })
        assertEquals(3L, sorted.first { it.name == "file2.txt" }.bytes)
        assertFalse(sorted.any { it.name == ".hidden" })
    }
    @Test fun conflictsPartialFailuresAndSelfDestination() = runBlocking {
        val repo = repo()
        write("note.txt", "source"); write("target/note.txt", "keep")
        val failures = repo.transferFiles(listOf("/home/brand/missing", "/home/brand/note.txt"), "/home/brand/target", false, FileConflict.KeepBoth, emptyList()) {}
        assertEquals(1, failures.size)
        assertEquals("keep", File(temporary.root, "target/note.txt").readText())
        assertEquals("source", File(temporary.root, "target/note (2).txt").readText())
        repo.transferFiles(listOf("/home/brand/note.txt"), "/home/brand/target", false, FileConflict.Skip, emptyList()) {}
        assertFalse(File(temporary.root, "target/note (3).txt").exists())
        assertEquals(1, repo.transferFiles(listOf("/home/brand/target"), "/home/brand/target", true, FileConflict.KeepBoth, emptyList()) {}.size)
    }
    @Test fun fileRenameAndFolderMoveUpdateLocations() = runBlocking {
        val repo = repo()
        write("source/note.txt", "hello"); File(temporary.root, "target").mkdir()
        repo.save("/home/brand/source", "Custom")
        repo.rename("/home/brand/source/note.txt", "renamed.txt", listOf("/home/brand/source"))
        assertTrue(repo.transferFiles(listOf("/home/brand/source"), "/home/brand/target", true, FileConflict.KeepBoth, emptyList()) {}.isEmpty())
        assertEquals("/home/brand/target/source", repo.locations().single().path)
        assertEquals("Custom", repo.locations().single().name)
        assertTrue(File(temporary.root, "target/source/renamed.txt").exists())
    }
    @Test fun snapshotsAreIndependentAndTextReadHasAnExplicitLimit() = runBlocking {
        val repo = repo()
        val original = write("note.txt", "original")
        val cache = File(temporary.root, "preview-cache")
        val snapshot = repo.snapshot("/home/brand/note.txt", cache)
        original.writeText("changed")
        assertEquals("original", snapshot.textPreview())
        snapshot.close(); assertFalse(snapshot.directory.exists())
        File(temporary.root, "large.txt").writeBytes(ByteArray(2 * 1024 * 1024 + 1) { 65 })
        repo.snapshot("/home/brand/large.txt", cache).use { large ->
            try { large.textPreview(); fail("Expected explicit limit") }
            catch (error: WorkspaceDirectoryException) { assertEquals(R.string.files_large_text, error.messageResource) }
        }
    }
    @Test fun cancelAndSymlinkFailureLeaveNoStagingArtifacts() = runBlocking {
        val repo = repo()
        write("source/note.txt", "hello"); File(temporary.root, "target").mkdir()
        Files.createSymbolicLink(File(temporary.root, "source/link").toPath(), temporary.root.toPath())
        assertEquals(1, repo.transferFiles(listOf("/home/brand/source"), "/home/brand/target", false, FileConflict.KeepBoth, emptyList()) {}.size)
        assertTrue(File(temporary.root, "target").listFiles()!!.isEmpty())
        val cancelled = launch(start = CoroutineStart.LAZY) { repo.files("/home/brand", true, "note", true); fail("Cancelled search must not execute") }
        cancelled.cancel(); cancelled.join()
    }
    @Test fun batchCancellationPreservesCompletedFiles() = runBlocking {
        val repo = repo(); write("first.txt", "one"); write("second.txt", "two"); File(temporary.root, "target").mkdir()
        val operation = async {
            repo.transferFiles(listOf("/home/brand/first.txt", "/home/brand/second.txt"), "/home/brand/target", false, FileConflict.KeepBoth, emptyList()) { progress ->
                if (progress.completed == 1) currentCoroutineContext().cancel()
            }
        }
        try { operation.await(); fail("Expected cancellation") } catch (_: CancellationException) { }
        assertTrue(File(temporary.root, "target/first.txt").exists())
        assertFalse(File(temporary.root, "target/second.txt").exists())
        assertTrue(File(temporary.root, "target").listFiles()!!.none { it.name.startsWith(".lingxi-copy-") })
    }
    @Test fun deleteFilePreservesItsProtectedParent() = runBlocking {
        val repo = repo(); write("note.txt", "hello")
        assertTrue(repo.deleteFiles(listOf("/home/brand/note.txt"), listOf("/home/brand")) {}.isEmpty())
        assertFalse(File(temporary.root, "note.txt").exists())
        assertTrue(temporary.root.isDirectory)
    }
}
