package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppWebViewTest {

    @Test
    fun `structured host script preserves semantic button and labelled textbox lookup`() {
        val clickRequest = buildLocalAppUiExecutionRequest(
            LocalAppUiAutomationAction.Click(
                LocalAppUiTarget(role = "button", name = "Save"),
            ),
        )
        assertTrue(clickRequest.contains("\"action\":\"click\""))
        assertTrue(clickRequest.contains("\"role\":\"button\""))
        assertTrue(clickRequest.contains("\"name\":\"Save\""))

        val fillRequest = buildLocalAppUiExecutionRequest(
            LocalAppUiAutomationAction.Fill(
                LocalAppUiTarget(role = "textbox", name = "Title"),
                value = "Orders",
            ),
        )
        assertTrue(fillRequest.contains("\"role\":\"textbox\""))
        assertTrue(fillRequest.contains("\"name\":\"Title\""))
        assertTrue(fillRequest.contains("\"value\":\"Orders\""))

        val inspectScript = buildLocalAppUiExecutionScript(
            buildLocalAppUiExecutionRequest(LocalAppUiAutomationAction.Inspect),
        )
        assertTrue(inspectScript.contains("BUTTON: 'button'"))
        assertTrue(inspectScript.contains("label[for=\"\${CSS.escape(element.id)}\"]"))
        assertTrue(inspectScript.contains("SELECT: 'combobox'"))
        assertTrue(inspectScript.contains("element.placeholder"))
    }

    @Test
    fun `ui execution parsing rejects false null and no result envelopes`() {
        assertEquals(
            "WebView UI automation returned no result",
            parseLocalAppUiExecutionResult("false").error,
        )
        assertEquals(
            "WebView UI automation returned no result",
            parseLocalAppUiExecutionResult("null").error,
        )
        assertEquals(
            "UI target was not found",
            parseLocalAppUiExecutionResult("""{"ok":false,"error":"UI target was not found"}""").error,
        )
        assertEquals(
            "WebView UI automation returned no result",
            parseLocalAppUiExecutionResult("""{"ok":true}""").error,
        )
    }

    @Test
    fun `ui execution parsing unwraps successful inspect payloads`() {
        val result = parseLocalAppUiExecutionResult(
            """{"ok":true,"result":{"elements":[{"role":"button","name":"Save"},{"role":"textbox","name":"Title"}]}}""",
        )

        assertNull(result.error)
        assertTrue(result.resultJson?.contains("Save") == true)
        assertTrue(result.resultJson?.contains("Title") == true)
    }
}
