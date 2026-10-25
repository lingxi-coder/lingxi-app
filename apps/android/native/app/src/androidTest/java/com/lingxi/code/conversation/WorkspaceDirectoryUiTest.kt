package com.lingxi.code.conversation

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionMode
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.project.WorkspaceDirectoryRepository
import com.lingxi.code.theme.LingXiTheme
import org.junit.*
import org.junit.Assert.*
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class WorkspaceDirectoryUiTest {
    @get:Rule val rule = createComposeRule()
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private lateinit var root: File
    @Before fun setup() { root = File(context.cacheDir, "workspace-ui-${UUID.randomUUID()}").apply { mkdirs() } }
    @After fun cleanup() { root.deleteRecursively() }
    private fun text(resource: Int) = context.getString(resource)

    @Test fun narrowToolbarAtLargeFontKeepsControlsOnOneRow() {
        rule.setContent {
            CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, fontScale = 2f)) {
                LingXiTheme(darkTheme = true) {
                    Column(Modifier.width(320.dp)) {
                        NewChatBar(SessionMode.Code, "A very long workspace folder name", {}, {})
                        Composer("", {}, EngineModelCatalog.pending, {}, {})
                    }
                }
            }
        }
        val bar = rule.onNodeWithTag(UiTags.NEW_CHAT_BAR).fetchSemanticsNode().boundsInRoot
        for (tag in listOf(UiTags.newChatMode("chat"), UiTags.newChatMode("code"), UiTags.NEW_CHAT_WORKSPACE)) {
            val bounds = rule.onNodeWithTag(tag).assertIsDisplayed().fetchSemanticsNode().boundsInRoot
            assertTrue(bounds.left >= bar.left && bounds.right <= bar.right + 1)
        }
        val toolbar = rule.onNodeWithTag("composer.toolbar").fetchSemanticsNode().boundsInRoot
        val attach = rule.onNodeWithTag(UiTags.COMPOSER_CAMERA).assertIsDisplayed().fetchSemanticsNode().boundsInRoot
        assertTrue(attach.top >= toolbar.top && attach.bottom <= toolbar.bottom + 1)
        capture("android-chat-toolbar-large-font")
    }

    @Test fun darkLargeTextDirectoryBrowserKeepsSelectionVisible() {
        File(root, "workspace/projects").mkdirs()
        val repo = WorkspaceDirectoryRepository(root, "/home/test", "/home/test/workspace", "/home/test/workspace/projects", File(root, ".config"))
        rule.setContent {
            CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, fontScale = 2f)) {
                LingXiTheme(darkTheme = true) {
                    WorkspaceDirectorySheet(emptyList(), ConversationScope.Global, repo.workspace, emptyList(), "Ready",
                        protectedPaths = { emptyList() }, canMutate = { true }, onDismiss = {}, onPick = {},
                        onPickDirectory = {}, repositoryOverride = repo)
                }
            }
        }
        rule.onNodeWithTag("workspace.contents").performScrollToNode(hasTestTag("workspace.browse"))
        capture("android-workspace-picker-dark-large-font")
        rule.onNodeWithTag("workspace.browse").performClick()
        rule.onNodeWithTag("workspace.choose").assertIsDisplayed().assertIsEnabled()
        capture("android-workspace-browser-dark-large-font")
    }

    private fun capture(name: String) {
        rule.waitForIdle()
        val bitmap = requireNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
        File(context.getExternalFilesDir(null), "$name.png").outputStream().use {
            bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
        }
    }

    @Test fun createSaveEditUnlinkAndConfirmDelete() {
        File(root, "workspace/projects").mkdirs()
        val repo = WorkspaceDirectoryRepository(root, "/home/test", "/home/test/workspace", "/home/test/workspace/projects", File(root, ".config"))
        val open = mutableStateOf(true)
        val current = mutableStateOf("/home/test/workspace")
        rule.setContent { LingXiTheme(darkTheme = false) {
            if (open.value) WorkspaceDirectorySheet(emptyList(), ConversationScope.Global, current.value, emptyList(), "Ready",
                protectedPaths = { listOf(current.value) }, canMutate = { true }, onDismiss = { open.value = false }, onPick = {},
                onPickDirectory = { current.value = it }, repositoryOverride = repo)
        } }
        capture("android-workspace-picker")
        rule.onNodeWithTag("workspace.browse").performClick()
        capture("android-workspace-browser")
        rule.onNodeWithTag("workspace.actions").performClick()
        rule.onNodeWithText(text(R.string.workspace_browser_new_folder)).performClick()
        rule.onNodeWithTag("workspace.edit.name").performTextInput("Example")
        rule.onNodeWithTag("workspace.edit.save").performClick()
        rule.waitUntil(5000) { File(root, "workspace/Example").isDirectory }
        rule.onNodeWithTag("workspace.folder.Example").performClick()
        rule.onNodeWithTag("workspace.choose").performClick()
        rule.waitUntil(5000) { !open.value }
        assertEquals("/home/test/workspace/Example", current.value)
        // Store a shortcut, then edit only its label and unlink it in the UI.
        repo.save(current.value, "Example")
        rule.runOnIdle { open.value = true }
        rule.onNodeWithTag("workspace.contents").performScrollToNode(hasText("Example"))
        val row = repo.locations().single()
        rule.onNodeWithTag("workspace.location.actions.${row.id}").performClick()
        rule.onNodeWithText(text(R.string.workspace_browser_edit_location)).performClick()
        rule.onNodeWithTag("workspace.edit.name").performTextReplacement("Alias")
        rule.onNodeWithTag("workspace.edit.save").performClick()
        rule.waitUntil(5000) { repo.locations().single().name == "Alias" }
        rule.onNodeWithTag("workspace.location.actions.${row.id}").performClick()
        rule.onNodeWithText(text(R.string.workspace_browser_remove_location)).performClick()
        rule.waitUntil(5000) { repo.locations().isEmpty() }
        assertTrue(File(root, "workspace/Example").exists())
        // Re-add and switch away so deleting the folder is allowed.
        repo.save(current.value, "Alias")
        rule.runOnIdle { open.value = false; current.value = "/home/test/workspace" }
        rule.waitForIdle()
        rule.runOnIdle { open.value = true }
        rule.onNodeWithTag("workspace.contents").performScrollToNode(hasText("Alias"))
        val saved = repo.locations().single()
        rule.onNodeWithTag("workspace.location.actions.${saved.id}").performClick()
        rule.onNodeWithText(text(R.string.workspace_browser_delete_folder)).performClick()
        rule.onNodeWithText(text(R.string.workspace_browser_delete_warning) + "\n\n/home/test/workspace/Example").assertExists()
        assertTrue(File(root, "workspace/Example").exists())
        rule.onAllNodesWithText(text(R.string.workspace_browser_delete_folder)).onLast().performClick()
        rule.waitUntil(5000) { !File(root, "workspace/Example").exists() }
        assertTrue(repo.locations().isEmpty())
    }
}
