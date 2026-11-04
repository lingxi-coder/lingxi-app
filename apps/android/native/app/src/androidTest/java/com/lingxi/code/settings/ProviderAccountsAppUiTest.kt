package com.lingxi.code.settings

import android.Manifest
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.Build
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.core.content.ContextCompat
import androidx.lifecycle.ViewModelProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import com.lingxi.code.theme.AppearanceStore
import java.io.File
import java.util.UUID
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.FixMethodOrder
import org.junit.rules.RuleChain
import org.junit.rules.TestRule
import org.junit.runner.Description
import org.junit.runner.RunWith
import org.junit.runners.model.Statement
import org.junit.runners.MethodSorters

/** Real MainActivity UI -> native account Host -> encrypted Android storage.
 * Run each method in its own instrumentation process, in order, on the offline fixture emulator.
 * No Test or Send action is invoked.
 */
@RunWith(AndroidJUnit4::class)
@FixMethodOrder(MethodSorters.NAME_ASCENDING)
class ProviderAccountsAppUiTest {
    private val compose = createAndroidComposeRule<MainActivity>()
    @get:Rule val rules: RuleChain = RuleChain.outerRule(ReadyForAccountSettings()).around(compose)
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private val marker get() = File(context.cacheDir, "provider-accounts-app-ui-fixture.json")
    private fun label(id: Int) = context.getString(id)

    private fun actualStore(): SettingsStore {
        lateinit var store: SettingsStore
        compose.runOnUiThread { store = ViewModelProvider(compose.activity)[SettingsStore::class.java] }
        return store
    }

    private fun openAccounts(store: SettingsStore, binding: String? = null) {
        compose.waitUntil(60_000) {
            compose.onAllNodesWithTag(UiTags.OPEN_DRAWER).fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithTag(UiTags.OPEN_DRAWER).performClick()
        compose.onNodeWithTag(UiTags.DRAWER_SETTINGS).performClick()
        compose.onNodeWithTag(UiTags.settingsGroup("models")).performScrollTo().performClick()
        compose.onNodeWithTag("provider.list.advanced").performScrollTo().performClick()
        compose.onNodeWithTag(UiTags.settingsPage(SettingsRoutes.PROVIDER_ACCOUNTS)).performScrollTo().performClick()
        try {
            compose.waitUntil(60_000) {
                store.providerAccounts.state.value.loaded && store.state.value.llmCatalogEntries.any {
                    if (binding != null) it.profileId == binding else it.profileId in listOf("openai", "anthropic", "gemini", "deepseek")
                }
            }
        } catch (timeout: androidx.compose.ui.test.ComposeTimeoutException) {
            lateinit var chat: com.lingxi.code.conversation.ChatViewModel
            compose.runOnUiThread { chat = ViewModelProvider(compose.activity)["chat", com.lingxi.code.conversation.ChatViewModel::class.java] }
            capture("bootstrap-not-ready")
            throw AssertionError("Real account Host not ready: source=${chat.engineSource.value.javaClass.simpleName}; chatError=${chat.state.value.error}; accountError=${store.providerAccounts.state.value.error}; catalogCount=${store.state.value.llmCatalogEntries.size}", timeout)
        }
        val entry = if (binding == null) {
            store.state.value.llmCatalogEntries.first { it.profileId in listOf("openai", "anthropic", "gemini", "deepseek") }
        } else store.state.value.llmCatalogEntries.first { it.profileId == binding }
        compose.onNodeWithText(entry.displayName).performScrollTo().performClick()
    }

    private fun formFields() = compose.onAllNodes(hasSetTextAction() and hasAnyAncestor(isDialog()))
    private fun capture(name: String) {
        val file = File(context.getExternalFilesDir("provider-accounts-app-ui"), "$name.png")
        file.parentFile!!.mkdirs()
        file.outputStream().use { output ->
            checkNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
                .compress(Bitmap.CompressFormat.PNG, 100, output)
        }
    }

    @Test fun phase1ActualAppCreatesRenamesAndDisablesSyntheticAccount() {
        val store = actualStore()
        openAccounts(store)
        assertFalse("Finish or remove only the earlier fixture before rerunning phase1", marker.exists())
        assertTrue("This UI acceptance requires an empty, isolated app account store", store.providerAccounts.state.value.accounts.isEmpty())
        val originalName = "UI fixture ${UUID.randomUUID()}"
        val finalName = "$originalName renamed"
        var ownedAccountId: String? = null
        var completed = false
        try {
            compose.onNodeWithText(label(R.string.provider_account_add_key)).performScrollTo().performClick()
            formFields().onFirst().performTextInput("synthetic-no-upstream-${UUID.randomUUID()}")
            compose.onNode(hasText(label(R.string.settings_section_advanced)) and hasAnyAncestor(isDialog())).performClick()
            formFields().onLast().performTextInput(originalName)
            compose.onNode(hasText(label(R.string.provider_account_add_key)) and hasAnyAncestor(isDialog())).performClick()
            compose.waitUntil(30_000) { store.providerAccounts.state.value.accounts.any { it.displayName == originalName } }
            val added = store.providerAccounts.state.value.accounts.single { it.displayName == originalName }
            ownedAccountId = added.accountId
            assertTrue(added.enabled)
            assertEquals("api_key", added.authKind)
            assertTrue(runBlocking { AndroidSecureStorageAdapter(context).list("provider-accounts.${store.providerAccounts.state.value.storeScope}") }
                .contains("acct.${added.accountId}"))
            compose.onNodeWithText(label(R.string.provider_account_edit)).performScrollTo().performClick()
            formFields().onFirst().performTextReplacement(finalName)
            compose.onNodeWithText(label(R.string.voice_save_button)).performClick()
            compose.waitUntil(30_000) { store.providerAccounts.state.value.accounts.single().displayName == finalName }
            compose.onNodeWithContentDescription("1. $finalName").performScrollTo().assertIsOn().performClick()
            compose.waitUntil(30_000) { !store.providerAccounts.state.value.accounts.single().enabled }
            compose.onNodeWithContentDescription("1. $finalName").assertIsOff()
            compose.onNodeWithText(label(R.string.provider_account_disabled)).assertExists()
            capture("created-renamed-disabled")
            marker.writeText(JSONObject().put("accountId", added.accountId).put("storeScope", store.providerAccounts.state.value.storeScope)
                .put("executionBindingId", added.executionBindingId).put("displayName", finalName).toString())
            completed = true
        } finally {
            if (!completed && ownedAccountId != null) {
                // The UI fixture owns this exact UUID only. Never remove unrelated accounts.
                runBlocking { store.providerAccounts.command("remove", JSONObject().put("accountId", ownedAccountId)) }
            }
        }
    }

    @Test fun phase2FreshProcessRestoresDisabledAccountAndRemovesItThroughActualUi() {
        assertTrue("Run phase1 in a separate instrumentation process first", marker.isFile)
        val fixture = JSONObject(marker.readText())
        val store = actualStore()
        openAccounts(store, fixture.getString("executionBindingId"))
        val state = store.providerAccounts.state.value
        assertEquals(fixture.getString("storeScope"), state.storeScope)
        val restored = state.accounts.single()
        assertEquals(fixture.getString("accountId"), restored.accountId)
        assertEquals(fixture.getString("displayName"), restored.displayName)
        assertFalse(restored.enabled)
        compose.onNodeWithContentDescription("1. ${restored.displayName}").performScrollTo().assertIsOff()
        capture("restored-after-process-restart")
        compose.onNodeWithText(label(R.string.provider_account_edit)).performScrollTo().performClick()
        compose.onNodeWithText(label(R.string.provider_account_remove)).performClick()
        val confirmation = context.getString(R.string.provider_account_remove_confirmation, restored.displayName)
        compose.onNode(hasText(label(R.string.provider_account_remove)) and hasClickAction() and
            hasAnyAncestor(isDialog() and hasAnyDescendant(hasText(confirmation)))).performClick()
        compose.waitUntil(30_000) { store.providerAccounts.state.value.accounts.none { it.accountId == restored.accountId } }
        assertFalse(runBlocking { AndroidSecureStorageAdapter(context).list("provider-accounts.${state.storeScope}") }
            .contains("acct.${restored.accountId}"))
        compose.onNodeWithText(label(R.string.provider_accounts_empty)).assertExists()
        capture("fixture-account-removed")
        assertTrue(marker.delete())
    }
}

/** Same first-run setup boundary used by AppFlowUiTest; native engine creation remains real. */
private class ReadyForAccountSettings : TestRule {
    override fun apply(base: Statement, description: Description) = object : Statement() {
        override fun evaluate() {
            val instrumentation = InstrumentationRegistry.getInstrumentation()
            val context = instrumentation.targetContext
            val appearance = AppearanceStore(context)
            val original = runBlocking { appearance.prefs.first() }
            runBlocking { appearance.setSetupDone(true) }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
            }
            try { base.evaluate() } finally { runBlocking { appearance.setSetupDone(original.setupDone) } }
        }
    }
}
