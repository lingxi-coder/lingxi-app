package com.lingxi.code.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Info
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ModelBillingModeDto
import com.lingxi.code.model.CatalogModelDetails
import kotlin.math.round

@Composable
fun ModelDetailsInfoButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    IconButton(onClick = onClick, modifier = modifier) {
        Icon(
            imageVector = Icons.Rounded.Info,
            contentDescription = stringResource(R.string.session_details_button),
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
fun ModelDetailsDialog(
    details: CatalogModelDetails,
    onDismiss: () -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    fun copy(id: Int, vararg args: Any): String = context.getString(id, *args)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(details.displayName, fontWeight = FontWeight.SemiBold)
                Text(
                    listOf(details.providerLabel, details.modelId)
                        .filter(String::isNotBlank)
                        .joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        text = {
            SelectionContainer {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(max = 440.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    details.description?.takeIf(String::isNotBlank)?.let { description ->
                        Text(
                            description,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    DetailRow(stringResource(R.string.model_detail_provider), details.providerLabel)
                    DetailRow(stringResource(R.string.model_detail_route), details.reference)
                    DetailRow(stringResource(R.string.settings_provider_model_id), details.modelId)
                    DetailRow(stringResource(R.string.model_detail_family), details.family)
                    DetailRow(stringResource(R.string.session_details_status), details.status)
                    DetailRow(stringResource(R.string.model_detail_release_date), details.releaseDate)
                    DetailRow(stringResource(R.string.model_detail_updated), details.lastUpdated)
                    DetailRow(stringResource(R.string.model_detail_knowledge_cutoff), details.knowledgeCutoff)
                    DetailRow(stringResource(R.string.model_detail_input_modalities), details.inputModalities.ifEmpty { null }?.joinToString(", "))
                    DetailRow(stringResource(R.string.model_detail_output_modalities), details.outputModalities.ifEmpty { null }?.joinToString(", "))
                    DetailRow(stringResource(R.string.model_detail_capabilities), details.capabilities.ifEmpty { null }?.joinToString(" · "))
                    DetailRow(stringResource(R.string.model_detail_max_context), details.contextWindowTokens?.let(::formatTokenCount))
                    DetailRow(stringResource(R.string.model_detail_input_budget), details.contextBudget?.effectiveInputBudget?.let(::formatTokenCount))
                    DetailRow(stringResource(R.string.model_detail_context_policy), details.contextBudget?.policy?.let { copy(when (it) { "long" -> R.string.composer_context_policy_long; "custom" -> R.string.composer_context_policy_custom; else -> R.string.composer_context_policy_auto }) })
                    DetailRow(stringResource(R.string.model_detail_standard_price_limit), details.contextBudget?.standardPriceInputTokens?.toString())
                    DetailRow(stringResource(R.string.model_detail_recommended_input), details.contextBudget?.recommendationInputTokens?.let(::formatTokenCount))
                    DetailRow(stringResource(R.string.model_detail_recommendation_source), details.contextBudget?.recommendationSource)
                    DetailRow(stringResource(R.string.model_detail_max_input), details.maxInputTokens?.let(::formatTokenCount))
                    DetailRow(stringResource(R.string.model_detail_max_output), details.maxOutputTokens?.let(::formatTokenCount))
                    DetailRow(
                        stringResource(R.string.model_detail_reasoning),
                        buildList {
                            if (details.reasoningOptions.isNotEmpty()) {
                                add(details.reasoningOptions.joinToString(" / ") { it.label })
                            }
                            details.reasoningDefault?.let { add("${copy(R.string.settings_badge_default)}: $it") }
                            if (details.reasoningForced) add(copy(R.string.model_detail_forced))
                            if (!details.reasoningEditable) add(copy(R.string.model_detail_locked))
                        }.ifEmpty { null }?.joinToString(" · "),
                    )
                    DetailRow(
                        stringResource(R.string.model_detail_pricing),
                        details.pricing?.let { pricing ->
                            when (pricing.billingMode) {
                                ModelBillingModeDto.SUBSCRIPTION -> copy(R.string.model_detail_subscription)
                                ModelBillingModeDto.FREE -> copy(R.string.model_detail_free)
                                ModelBillingModeDto.UNKNOWN -> copy(R.string.model_detail_no_price)
                                ModelBillingModeDto.PER_TOKEN -> buildList {
                                    pricing.inputPerMillion?.let { add(copy(R.string.model_detail_price_input_fmt, formatPrice(it)) + " / 1M") }
                                    pricing.outputPerMillion?.let { add(copy(R.string.model_detail_price_output_fmt, formatPrice(it)) + " / 1M") }
                                    pricing.cacheReadPerMillion?.let { add(copy(R.string.model_detail_price_cache_read_fmt, formatPrice(it)) + " / 1M") }
                                    pricing.cacheWritePerMillion?.let { add(copy(R.string.model_detail_price_cache_write_fmt, formatPrice(it)) + " / 1M") }
                                    pricing.reasoningPerMillion?.let { add(copy(R.string.model_detail_price_reasoning_fmt, formatPrice(it)) + " / 1M") }
                                }.ifEmpty { listOf(copy(R.string.model_detail_no_price)) }.joinToString("\n")
                            }
                        },
                    )
                    details.pricing?.tiers?.takeIf { it.isNotEmpty() }?.let { tiers ->
                        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(
                                stringResource(R.string.model_detail_pricing_tiers),
                                fontSize = androidx.compose.material3.MaterialTheme.typography.bodySmall.fontSize,
                                fontWeight = FontWeight.SemiBold,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            tiers.forEach { tier ->
                                Text(
                                    copy(R.string.model_detail_price_range_fmt, tier.contextThresholdTokens.toString(), tier.maxInputTokens?.toString() ?: "∞") + ": " +
                                        listOfNotNull(
                                            tier.inputPerMillion?.let { copy(R.string.model_detail_price_input_fmt, formatPrice(it)) },
                                            tier.outputPerMillion?.let { copy(R.string.model_detail_price_output_fmt, formatPrice(it)) },
                                            tier.cacheReadPerMillion?.let { copy(R.string.model_detail_price_cache_read_fmt, formatPrice(it)) },
                                            tier.cacheWritePerMillion?.let { copy(R.string.model_detail_price_cache_write_fmt, formatPrice(it)) },
                                            tier.reasoningPerMillion?.let { copy(R.string.model_detail_price_reasoning_fmt, formatPrice(it)) },
                                        ).joinToString(" · "),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurface,
                                )
                            }
                        }
                    }
                    DetailRow(stringResource(R.string.model_detail_attachments), details.attachments?.let { copy(if (it) R.string.common_yes else R.string.common_no) })
                    DetailRow(stringResource(R.string.model_detail_open_weights), details.openWeights?.let { copy(if (it) R.string.common_yes else R.string.common_no) })
                    DetailRow(stringResource(R.string.model_detail_temperature), details.temperatureControl?.let { copy(if (it) R.string.common_yes else R.string.common_no) })
                    DetailRow(stringResource(R.string.audio_source_title), details.pricing?.source)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.common_close))
            }
        },
    )
}

@Composable
private fun DetailRow(label: String, value: String?) {
    if (value.isNullOrBlank()) return
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Text(
            label,
            modifier = Modifier.weight(0.35f),
            fontSize = androidx.compose.material3.MaterialTheme.typography.bodySmall.fontSize,
            fontWeight = FontWeight.SemiBold,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            value,
            modifier = Modifier.weight(0.65f),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurface,
        )
    }
}

private fun formatTokenCount(value: ULong): String {
    val n = value.toDouble()
    return when {
        n >= 1_000_000.0 -> "${trimDecimal(n / 1_000_000.0)}M"
        n >= 1_000.0 -> "${trimDecimal(n / 1_000.0)}K"
        else -> value.toString()
    }
}

private fun formatPrice(value: Double): String = trimDecimal(value)

private fun trimDecimal(value: Double): String {
    val rounded = round(value * 100.0) / 100.0
    return if (rounded % 1.0 == 0.0) rounded.toInt().toString() else rounded.toString()
}
