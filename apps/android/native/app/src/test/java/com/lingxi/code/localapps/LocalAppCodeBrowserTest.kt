package com.lingxi.code.localapps

import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Test

class LocalAppCodeBrowserTest {
    @Test
    fun `lists editable source but excludes private generated and symlink content`() {
        val root = Files.createTempDirectory("local-app-browser").toFile()
        try {
            val workspace = root.resolve("apps/test-app/workspace").apply { mkdirs() }
            workspace.resolve("app/page.tsx").apply { parentFile!!.mkdirs(); writeText("export default 1") }
            workspace.resolve(".lingxi/design-spec.json").apply { parentFile!!.mkdirs(); writeText("{}") }
            workspace.resolve("public/image.bin").apply { parentFile!!.mkdirs(); writeBytes(byteArrayOf(1)) }
            val outside = root.resolve("outside.ts").apply { writeText("secret") }
            Files.createSymbolicLink(workspace.resolve("app/link.ts").toPath(), outside.toPath())

            val browser = LocalAppCodeBrowser(root, "apps/test-app/workspace")
            assertEquals(listOf("app/page.tsx"), browser.listFiles().map { it.relativePath })
            assertFalse(browser.listFiles().any { it.relativePath.contains("lingxi") })
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun `rejects escaped workspaces and files`() {
        val root = Files.createTempDirectory("local-app-browser").toFile()
        try {
            root.resolve("apps/test-app/workspace/app").mkdirs()
            val browser = LocalAppCodeBrowser(root, "apps/test-app/workspace")
            assertThrows(IllegalArgumentException::class.java) { LocalAppCodeBrowser(root, "../outside") }
            assertThrows(IllegalArgumentException::class.java) { browser.read("../../outside.ts") }
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun `saves UTF-8 source atomically`() {
        val root = Files.createTempDirectory("local-app-browser").toFile()
        try {
            val source = root.resolve("apps/test-app/workspace/app/page.tsx")
                .apply { parentFile!!.mkdirs(); writeText("before") }
            val browser = LocalAppCodeBrowser(root, "apps/test-app/workspace")
            browser.save("app/page.tsx", "之后")
            assertEquals("之后", source.readText())
        } finally {
            root.deleteRecursively()
        }
    }
}
