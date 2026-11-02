package com.lingxi.code.conversation

import com.lingxi.code.settings.ProviderAccountModelAvailability
import com.lingxi.code.settings.findModelAvailability
import org.junit.Assert.*
import org.junit.Test

class RemoteModelSelectionTest {
    private val row = ProviderAccountModelAvailability("openai", "openai-chatgpt", "org/model-id",
        listOf("host-oauth-account"), emptyList(), listOf("host-oauth-account"), emptyList())

    @Test fun executionHostProjectionDoesNotRequirePhoneProviderCredentials() {
        val actual = findModelAvailability(listOf(row), "openai-chatgpt/org/model-id")
        assertTrue(requireNotNull(actual).hasEligibleAccounts)
        assertFalse(row.copy(eligibleAccountIds = emptyList(), confirmedAccountIds = listOf("historical-account")).hasEligibleAccounts)
    }

    @Test fun absentProjectionAndDifferentBindingOrModelCannotAuthorizeSelection() {
        assertNull(findModelAvailability(emptyList(), "openai-chatgpt/org/model-id"))
        assertNull(findModelAvailability(listOf(row), "openai/org/model-id"))
        assertNull(findModelAvailability(listOf(row), "openai-chatgpt/model-id"))
        assertNull(findModelAvailability(listOf(row), "openai-chatgpt/Model ID"))
    }
}
