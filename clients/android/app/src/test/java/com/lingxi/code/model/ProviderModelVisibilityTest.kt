package com.lingxi.code.model

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class ProviderModelVisibilityTest {

    @Test
    fun visibleCatalogModels_defaultsToAllAndHonorsExplicitEmptyAllowlist() {
        val models = listOf(
            details("openai/gpt-5.6-sol", "openai", "gpt-5.6-sol"),
            details("openai/gpt-5.7-preview", "openai", "gpt-5.7-preview"),
        )

        assertEquals(
            models.map { it.modelId },
            ProviderModelVisibilityRules.visibleCatalogModels(models, null).map { it.modelId },
        )
        assertEquals(
            emptyList<String>(),
            ProviderModelVisibilityRules.visibleCatalogModels(
                models,
                ProviderModelVisibility(showInModelPicker = true, visibleModelIds = emptyList()),
            ).map { it.modelId },
        )
    }

    @Test
    fun visibleConversationModels_hidesProviderSwitchAndPreservesUnknownProviders() {
        val models = listOf(
            option("openai/gpt-5.6-sol", "openai", "gpt-5.6-sol"),
            option("openai/gpt-5.7-preview", "openai", "gpt-5.7-preview"),
            option("deepseek/deepseek-v4-flash", "deepseek", "deepseek-v4-flash"),
            option("community/custom-model", "community", "custom-model"),
        )

        val filtered = ProviderModelVisibilityRules.visibleConversationModels(
            models,
            mapOf(
                "openai" to ProviderModelVisibility(visibleModelIds = listOf("gpt-5.6-sol")),
                "deepseek" to ProviderModelVisibility(showInModelPicker = false),
            ),
        )

        assertEquals(
            listOf("openai/gpt-5.6-sol", "community/custom-model"),
            filtered.map { it.id },
        )
    }

    @Test
    fun visibleConversationModels_doesNotApplyCatalogMembershipBeforeCatalogLoads() {
        val models = listOf(
            option("openai/gpt-5.6-sol", "openai", "gpt-5.6-sol"),
            option("openai/gpt-5.7-preview", "openai", "gpt-5.7-preview"),
        )

        assertEquals(
            models.map { it.id },
            ProviderModelVisibilityRules.visibleConversationModels(
                models,
                mapOf(
                    "openai" to ProviderModelVisibility(
                        catalogModelIds = null,
                        showInModelPicker = true,
                        visibleModelIds = null,
                    ),
                ),
            ).map { it.id },
        )
    }

    @Test
    fun visibleConversationModels_requiresExactCatalogMembershipAfterCatalogLoads() {
        val models = listOf(
            option("openai/gpt-5.6-sol", "openai", "gpt-5.6-sol"),
            option("openai/gpt-5.7-preview", "openai", "gpt-5.7-preview"),
            option("deepseek/deepseek-v4-flash", "deepseek", "deepseek-v4-flash"),
        )

        assertEquals(
            listOf("openai/gpt-5.6-sol", "deepseek/deepseek-v4-flash"),
            ProviderModelVisibilityRules.visibleConversationModels(
                models,
                mapOf(
                    "openai" to ProviderModelVisibility(
                        catalogModelIds = listOf("openai/gpt-5.6-sol"),
                        showInModelPicker = true,
                        visibleModelIds = null,
                    ),
                    "deepseek" to ProviderModelVisibility(
                        catalogModelIds = listOf("deepseek-v4-flash"),
                        showInModelPicker = true,
                        visibleModelIds = null,
                    ),
                ),
            ).map { it.id },
        )
    }

    @Test
    fun selectedCatalogModelIds_preservesStoredSelectionWhenMasterSwitchIsOff() {
        val candidateIds = listOf("gpt-5.6-sol", "gpt-5.7-preview")

        assertEquals(
            listOf("gpt-5.6-sol"),
            ProviderModelVisibilityRules.selectedCatalogModelIds(
                candidateIds,
                ProviderModelVisibility(
                    showInModelPicker = false,
                    visibleModelIds = listOf("gpt-5.6-sol"),
                ),
            ),
        )
        assertEquals(
            candidateIds,
            ProviderModelVisibilityRules.selectedCatalogModelIds(
                candidateIds,
                ProviderModelVisibility(
                    showInModelPicker = false,
                    visibleModelIds = null,
                ),
            ),
        )
        assertFalse(
            ProviderModelVisibilityRules.visibleConversationModels(
                listOf(
                    option("openai/gpt-5.6-sol", "openai", "gpt-5.6-sol"),
                    option("openai/gpt-5.7-preview", "openai", "gpt-5.7-preview"),
                ),
                mapOf(
                    "openai" to ProviderModelVisibility(
                        showInModelPicker = false,
                        visibleModelIds = listOf("gpt-5.6-sol"),
                    ),
                ),
            ).any { it.id == "openai/gpt-5.6-sol" },
        )
    }

    private fun option(
        reference: String,
        providerId: String,
        modelId: String,
    ) = ModelOption(
        id = reference,
        name = modelId,
        desc = modelId,
        tag = "",
        color = Color.Black,
        providerId = providerId,
        providerName = providerId,
        details = details(reference, providerId, modelId),
    )

    private fun details(
        reference: String,
        providerId: String,
        modelId: String,
    ) = CatalogModelDetails(
        reference = reference,
        providerId = providerId,
        providerLabel = providerId,
        displayName = modelId,
        modelId = modelId,
        description = null,
        family = null,
        status = null,
        releaseDate = null,
        lastUpdated = null,
        knowledgeCutoff = null,
        inputModalities = emptyList(),
        outputModalities = emptyList(),
        contextWindowTokens = null,
        maxInputTokens = null,
        maxOutputTokens = null,
        openWeights = null,
        attachments = null,
        temperatureControl = null,
        pricing = null,
        capabilities = emptyList(),
        reasoningOptions = emptyList(),
        reasoningEditable = true,
        reasoningForced = false,
        reasoningDefault = null,
        metadata = ModelMetadata(modelId = modelId),
    )
}
