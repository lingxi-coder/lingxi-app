package com.lingxi.code.project

import com.lingxi.code.R
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files

class WorkspaceDirectoryRepositoryTest {
    @get:Rule val temp = TemporaryFolder()
    private fun repository(): WorkspaceDirectoryRepository {
        File(temp.root, "workspace/projects").mkdirs()
        return WorkspaceDirectoryRepository(temp.root, "/home/brand", "/home/brand/workspace",
            "/home/brand/workspace/projects", File(temp.root, ".config"))
    }
    @Test fun createPickEditUnlinkPreservesFilesAndCanonicalIdentity() {
        val repo = repository()
        val path = repo.create("/home/brand/workspace", "example")
        repo.save(path, "Example")
        val row = repo.locations().single()
        repo.save(path, "Renamed label", row.id)
        repo.save(path, "Do not replace label")
        assertEquals("Renamed label", repo.locations().single().name)
        repo.unlink(row.id)
        assertTrue(repo.locations().isEmpty())
        assertTrue(repo.resolve(path).isDirectory)
        assertEquals(listOf("example", "projects"), repo.entries("/home/brand/workspace", false).map { it.name })
    }
    @Test fun renameUpdatesNestedLocationsAndDeleteRemovesTreeWithoutFollowingLinks() {
        val repo = repository()
        val path = repo.create("/home/brand/workspace", "a")
        val nested = repo.create(path, "nested")
        repo.save(nested, "Keep name")
        repo.rename(path, "b", emptyList())
        val next = "/home/brand/workspace/b"
        assertEquals("$next/nested", repo.locations().single().path)
        val outside = temp.newFolder("outside")
        File(outside, "keep").writeText("user data")
        Files.createSymbolicLink(File(repo.resolve(next), "link").toPath(), outside.toPath())
        repo.delete(next, emptyList())
        assertTrue(File(outside, "keep").exists())
        assertTrue(repo.locations().isEmpty())
        assertFalse(File(temp.root, "workspace/b").exists())
    }
    @Test fun protectsActiveDirectoryAncestorsAndRuntimeRoots() {
        val repo = repository()
        val path = repo.create("/home/brand/workspace", "active")
        val nested = repo.create(path, "nested")
        for (target in listOf("/home/brand", "/home/brand/workspace", "/home/brand/workspace/projects", path)) {
            fails(R.string.workspace_browser_in_use) { repo.delete(target, listOf(nested)) }
        }
        fails(R.string.workspace_browser_in_use) { repo.rename(path, "renamed", listOf(nested)) }
    }
    @Test fun rejectsTraversalEscapingLinksAndInvalidNames() {
        val repo = repository()
        fails(R.string.workspace_browser_outside_home) { repo.resolve("/home/brand/../outside") }
        fails(R.string.workspace_browser_outside_home) { repo.resolve("/root") }
        val outside = Files.createTempDirectory("lingxi-workspace-outside").toFile()
        try {
            Files.createSymbolicLink(File(temp.root, "escape").toPath(), outside.toPath())
            fails(R.string.workspace_browser_outside_home) { repo.resolve("/home/brand/escape") }
        } finally { outside.delete() }
        for (name in listOf("..", "a/b", "\u0000bad")) fails(R.string.workspace_browser_invalid_name) {
            repo.create("/home/brand/workspace", name)
        }
    }
    @Test fun aliasesCannotBypassConfigOrActiveFolderProtection() {
        val repo = repository()
        val active = repo.create("/home/brand/workspace", "active")
        Files.createSymbolicLink(File(temp.root, "alias").toPath(), repo.resolve(active).toPath())
        fails(R.string.workspace_browser_in_use) { repo.delete("/home/brand/alias", emptyList()) }
        fails(R.string.workspace_browser_in_use) { repo.rename(active, "other", listOf("/home/brand/alias")) }
        repo.save(active, "Custom")
        Files.createSymbolicLink(File(temp.root, "config-alias").toPath(), File(temp.root, ".config").toPath())
        fails(R.string.workspace_browser_in_use) { repo.create("/home/brand/config-alias", "bad") }
        repo.save("/home/brand/alias", "Other")
        assertEquals("Custom", repo.locations().single().name)
    }
    @Test fun linkedSavedLocationStoreCannotOverwriteAnotherFile() {
        val repo = repository()
        val config = File(temp.root, ".config").apply { mkdir() }
        val original = temp.newFile("original").apply { writeText("keep") }
        Files.createSymbolicLink(File(config, "workspace-locations.properties").toPath(), original.toPath())
        fails(R.string.workspace_browser_outside_home) { repo.save("/home/brand/workspace", "workspace") }
        assertEquals("keep", original.readText())
    }
    private fun fails(resource: Int, action: () -> Unit) {
        try { action(); fail("Expected protected operation to fail") }
        catch (error: WorkspaceDirectoryException) { assertEquals(resource, error.messageResource) }
    }
}
