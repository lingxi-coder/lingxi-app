package com.lingxi.code.localapps

import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Test

class LocalAppWorkspaceTest {
    @Test
    fun `workspace rel for another app falls back to the requested app id`() {
        val root = Files.createTempDirectory("local-app-workspace").toFile()
        try {
            root.resolve("apps/app-b/workspace").mkdirs()

            val resolved = localAppWorkspace(root, "app-a", "apps/app-b/workspace")

            assertEquals(
                root.resolve("apps/app-a/workspace").absolutePath,
                resolved.hostPath,
            )
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun `matching workspace rel is retained`() {
        val root = Files.createTempDirectory("local-app-workspace").toFile()
        try {
            val resolved = localAppWorkspace(root, "app-a", "apps/app-a/workspace")

            assertEquals(
                root.resolve("apps/app-a/workspace").absolutePath,
                resolved.hostPath,
            )
        } finally {
            root.deleteRecursively()
        }
    }
}
