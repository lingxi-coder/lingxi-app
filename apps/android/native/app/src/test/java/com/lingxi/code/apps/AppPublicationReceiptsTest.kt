package com.lingxi.code.apps

import java.io.File
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class AppPublicationReceiptsTest {
    @get:Rule val temporary = TemporaryFolder()
    @Test fun committedIdentitiesSurvivePublisherRecreation() {
        val root = temporary.newFolder()
        AppPublicationReceipts(root).save("release:createVersion", "fixed-key")
        AppPublicationReceipts(root).save("release:versionBody", "original notes")
        val recreated = AppPublicationReceipts(root)
        assertEquals("fixed-key", recreated.getString("release:createVersion", null))
        assertEquals("original notes", recreated.getString("release:versionBody", null))
    }
    @Test fun failedWriteCannotCreateAnInMemoryIdentity() {
        val root = temporary.newFolder()
        val receipts = AppPublicationReceipts(root)
        assertTrue(root.delete()); assertTrue(root.createNewFile())
        assertThrows(Exception::class.java) { receipts.save("release:key", "must-not-survive") }
        assertTrue(root.delete()); assertTrue(root.mkdir())
        assertFalse(receipts.contains("release:key"))
        assertNull(receipts.getString("release:key", null))
    }
    @Test fun corruptionIsPreservedAndStopsPublication() {
        val root = temporary.newFolder()
        val file = File(root, "receipts.json")
        file.writeText("broken journal")
        val receipts = AppPublicationReceipts(root)
        assertThrows(Exception::class.java) { receipts.getString("release:key", null) }
        assertThrows(Exception::class.java) { receipts.save("release:key", "replacement") }
        assertEquals("broken journal", file.readText())
    }
}
