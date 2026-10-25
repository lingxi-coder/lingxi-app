package com.lingxi.code.conversation

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.project.WorkspaceDirectoryRepository
import com.lingxi.code.theme.LingXiTheme
import org.junit.*
import org.junit.Assert.*
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class WorkspaceFilesUiTest {
    @get:Rule val rule = createComposeRule()
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private lateinit var root: File
    private lateinit var repository: WorkspaceDirectoryRepository
    @Before fun setup() {
        root = File(context.cacheDir, "files-ui-${UUID.randomUUID()}").apply { mkdirs() }
        File(root, "target").mkdir()
        File(root, "note2.txt").writeText("Read-only file preview ✓")
        File(root, "note10.txt").writeText("another file")
        File(root, ".hidden").writeText("hidden")
        File(root, "large.txt").writeBytes(ByteArray(2 * 1024 * 1024 + 1) { 65 })
        File(root, "unknown.bin").writeBytes(byteArrayOf(0, 1, 2))
        repository = WorkspaceDirectoryRepository(root, "/home/test", "/home/test/work", "/home/test/work/projects", File(root, ".config"))
        val bitmap = android.graphics.Bitmap.createBitmap(40, 40, android.graphics.Bitmap.Config.ARGB_8888).apply { eraseColor(android.graphics.Color.BLUE) }
        File(root, "image.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
        val wav = java.nio.ByteBuffer.allocate(44 + 8000).order(java.nio.ByteOrder.LITTLE_ENDIAN)
        wav.put("RIFF".toByteArray()).putInt(36 + 8000).put("WAVEfmt ".toByteArray()).putInt(16)
            .putShort(1).putShort(1).putInt(8000).putInt(8000).putShort(1).putShort(8)
            .put("data".toByteArray()).putInt(8000)
        while (wav.hasRemaining()) wav.put(128.toByte())
        File(root, "sound.wav").writeBytes(wav.array())
        val document = android.graphics.pdf.PdfDocument()
        try {
            val page = document.startPage(android.graphics.pdf.PdfDocument.PageInfo.Builder(200, 200, 1).create())
            page.canvas.drawText("PDF preview", 20f, 30f, android.graphics.Paint())
            document.finishPage(page); File(root, "document.pdf").outputStream().use { document.writeTo(it) }
        } finally { document.close() }
    }
    @After fun cleanup() { root.deleteRecursively() }
    private fun text(resource: Int) = context.getString(resource)
    private fun launch() { rule.setContent { LingXiTheme(darkTheme = false) {
        WorkspaceFilesDialog(repository, "/home/test", { emptyList() }, { true }, {})
    } }; rule.waitUntil(5000) { rule.onAllNodesWithTag("files.entry.note2.txt").fetchSemanticsNodes().isNotEmpty() } }
    @Test fun searchGridPreviewAndLargeTextLimit() {
        launch()
        rule.onNodeWithTag("files.search").performTextInput("note2")
        rule.onNodeWithTag("files.entry.note10.txt").assertDoesNotExist()
        rule.onNodeWithTag("files.entry.note2.txt").performClick()
        rule.onNodeWithText("Read-only file preview ✓").assertExists()
        rule.onNodeWithTag("files.preview.close").performClick()
        rule.onNodeWithTag("files.search").performTextClearance()
        rule.onNodeWithTag("files.actions").performClick()
        rule.onNodeWithText(text(R.string.files_grid)).performClick()
        rule.onNodeWithTag("files.grid").assertExists()
        capture("android-files-grid")
        rule.onNodeWithTag("files.entry.large.txt").performClick()
        rule.onNodeWithText(text(R.string.files_large_text)).assertExists()
    }
    @Test fun copyRenameMoveAndConfirmedDelete() {
        launch()
        rule.onNodeWithTag("files.entry.actions.note2.txt").performClick()
        rule.onNodeWithText(text(R.string.files_copy)).performClick()
        rule.onAllNodesWithTag("files.entry.target").onLast().performClick()
        rule.onNodeWithTag("workspace.choose").performClick()
        rule.onNodeWithText(text(R.string.files_keep_both)).performClick()
        rule.waitUntil(5000) { File(root, "target/note2.txt").exists() }
        assertEquals("Read-only file preview ✓", File(root, "target/note2.txt").readText())
        rule.onNodeWithTag("files.entry.actions.note2.txt").performClick()
        rule.onNodeWithText(text(R.string.files_rename)).performClick()
        rule.onNodeWithTag("files.name").performTextReplacement("renamed.txt")
        rule.onNodeWithText(text(R.string.files_save)).performClick()
        rule.waitUntil(5000) { File(root, "renamed.txt").exists() }
        rule.onNodeWithTag("files.entry.actions.renamed.txt").performClick()
        rule.onNodeWithText(text(R.string.files_move)).performClick()
        rule.onAllNodesWithTag("files.entry.target").onLast().performClick()
        rule.onNodeWithTag("workspace.choose").performClick()
        rule.onNodeWithText(text(R.string.files_keep_both)).performClick()
        rule.waitUntil(5000) { File(root, "target/renamed.txt").exists() && !File(root, "renamed.txt").exists() }
        rule.onNodeWithTag("files.entry.actions.note10.txt").performClick()
        rule.onNodeWithText(text(R.string.files_delete)).performClick()
        assertTrue(File(root, "note10.txt").exists())
        rule.onAllNodesWithText(text(R.string.files_delete)).onLast().performClick()
        rule.waitUntil(5000) { !File(root, "note10.txt").exists() }
        capture("android-files-list")
    }
    @Test fun imagePdfAndUnknownPreviews() {
        launch()
        for (name in listOf("image.png", "document.pdf", "sound.wav", "unknown.bin")) {
            rule.onNodeWithTag("files.list").performScrollToNode(hasTestTag("files.entry.$name"))
            rule.onNodeWithTag("files.entry.$name").performClick()
            rule.onNodeWithTag("files.preview").assertExists()
            if (name == "unknown.bin") rule.onNodeWithText(text(R.string.files_unsupported)).assertExists()
            capture("android-files-preview-$name")
            rule.onNodeWithTag("files.preview.close").performClick()
        }
    }
    @Test fun nativeImportExportAndSharePanels() {
        launch()
        val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
        val hostPackages = setOf(context.packageName, InstrumentationRegistry.getInstrumentation().context.packageName)
        fun waitForSystemPanel() {
            // Compose v2 can host its activity in the test APK. It is still an app window.
            rule.waitUntil(8000) { automation.rootInActiveWindow?.packageName?.toString()?.let { it !in hostPackages } == true }
            automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            rule.waitUntil(8000) { automation.rootInActiveWindow?.packageName?.toString() in hostPackages }
        }
        rule.onNodeWithTag("files.actions").performClick()
        rule.onNodeWithText(text(R.string.files_import_files)).performClick()
        waitForSystemPanel()
        rule.onNodeWithTag("files.entry.actions.note2.txt").performClick()
        rule.onNodeWithText(text(R.string.files_export)).performClick()
        waitForSystemPanel()
        rule.onNodeWithTag("files.entry.actions.note2.txt").performClick()
        rule.onNodeWithText(text(R.string.files_share)).performClick()
        waitForSystemPanel()
        assertEquals("Read-only file preview ✓", File(root, "note2.txt").readText())
    }
    @Test fun conversationMenuOpensFiles() {
        val open = androidx.compose.runtime.mutableStateOf(false)
        rule.setContent { LingXiTheme(darkTheme = false) {
            ChatScreen(state = ChatState(session = com.lingxi.code.model.SessionRef("test", "Session name"), messages = emptyList(), model = com.lingxi.code.model.EngineModelCatalog.pending),
                onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false, onOpenFiles = { open.value = true })
            if (open.value) WorkspaceFilesDialog(repository, "/home/test", { emptyList() }, { true }, { open.value = false })
        } }
        rule.onNodeWithTag("conversation.more").performClick()
        rule.onNodeWithTag("conversation.files").performClick()
        rule.onNodeWithTag("files.search").assertExists()
    }
    private fun capture(name: String) {
        rule.waitForIdle()
        android.os.SystemClock.sleep(400) // Allow native popup/window transitions to finish before capturing.
        val bitmap = requireNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
        File(context.getExternalFilesDir(null), "$name.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
    }
}
