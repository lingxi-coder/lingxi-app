package com.lingxi.code.conversation

import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.ModelProviderStatus
import org.junit.Assert.*
import org.junit.Test

class RemoteModelSelectionTest {
    private val configured = ModelProviderStatus(
        "provider", "provider", "Provider", ConnStatus.Configured,
        enabled = true, credentialConfigured = true,
    )

    @Test fun remoteCatalogDoesNotRequirePhoneCredentials() {
        assertTrue(modelPickerCanSelect(remote = true, provider = null))
        assertTrue(modelPickerCanSelect(remote = true, provider = configured.copy(enabled = false)))
    }

    @Test fun localCatalogRetainsProviderReadinessGate() {
        assertFalse(modelPickerCanSelect(remote = false, provider = null))
        assertFalse(modelPickerCanSelect(remote = false, provider = configured.copy(enabled = false)))
        assertFalse(modelPickerCanSelect(remote = false, provider = configured.copy(status = ConnStatus.Idle, credentialConfigured = false)))
        assertTrue(modelPickerCanSelect(remote = false, provider = configured))
    }
}
