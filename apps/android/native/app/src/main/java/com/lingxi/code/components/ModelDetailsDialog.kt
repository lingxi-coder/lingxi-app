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
                    DetailRow("Provider", details.providerLabel)
                    DetailRow("Reference", details.reference)
                    DetailRow("Raw model ID", details.modelId)
                    DetailRow("Family", details.family)
                    DetailRow("Status", details.status)
                    DetailRow("Release", details.releaseDate)
                    DetailRow("Updated", details.lastUpdated)
                    DetailRow("Knowledge", details.knowledgeCutoff)
                    DetailRow("Input modalities", details.inputModalities.ifEmpty { null }?.joinToString(", "))
                    DetailRow("Output modalities", details.outputModalities.ifEmpty { null }?.joinToString(", "))
                    DetailRow("Capabilities", details.capabilities.ifEmpty { null }?.joinToString(" · "))
                    DetailRow("Context window", details.contextWindowTokens?.let(::formatTokenCount))
                    DetailRow("Max input", details.maxInputTokens?.let(::formatTokenCount))
                    DetailRow("Max output", details.maxOutputTokens?.let(::formatTokenCount))
                    DetailRow(
                        "Reasoning",
                        buildList {
                            if (details.reasoningOptions.isNotEmpty()) {
                                add(details.reasoningOptions.joinToString(" / ") { it.label })
                            }
                            details.reasoningDefault?.let { add("default: $it") }
                            if (details.reasoningForced) add("forced")
                            if (!details.reasoningEditable) add("locked")
                        }.ifEmpty { null }?.joinToString(" · "),
                    )
                    DetailRow(
                        "Pricing",
                        details.pricing?.let { pricing ->
                            when (pricing.billingMode) {
                                ModelBillingModeDto.SUBSCRIPTION -> "套餐/订阅内"
                                ModelBillingModeDto.FREE -> "免费"
                                ModelBillingModeDto.UNKNOWN -> "价格未提供"
                                ModelBillingModeDto.PER_TOKEN -> buildList {
                                    pricing.inputPerMillion?.let { add("input \$${formatPrice(it)} / 1M") }
                                    pricing.outputPerMillion?.let { add("output \$${formatPrice(it)} / 1M") }
                                    pricing.cacheReadPerMillion?.let { add("cache read \$${formatPrice(it)} / 1M") }
                                    pricing.cacheWritePerMillion?.let { add("cache write \$${formatPrice(it)} / 1M") }
                                    pricing.reasoningPerMillion?.let { add("reasoning \$${formatPrice(it)} / 1M") }
                                }.ifEmpty { listOf("价格未提供") }.joinToString("\n")
                            }
                        },
                    )
                    details.pricing?.tiers?.takeIf { it.isNotEmpty() }?.let { tiers ->
                        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(
                                "Pricing tiers",
                                fontSize = 12.sp,
                                fontWeight = FontWeight.SemiBold,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            tiers.forEach { tier ->
                                Text(
                                    "≥ ${formatTokenCount(tier.contextThresholdTokens)}: " +
                                        listOfNotNull(
                                            tier.inputPerMillion?.let { "input \$${formatPrice(it)}" },
                                            tier.outputPerMillion?.let { "output \$${formatPrice(it)}" },
                                            tier.cacheReadPerMillion?.let { "cache read \$${formatPrice(it)}" },
                                            tier.cacheWritePerMillion?.let { "cache write \$${formatPrice(it)}" },
                                            tier.reasoningPerMillion?.let { "reasoning \$${formatPrice(it)}" },
                                        ).joinToString(" · "),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurface,
                                )
                            }
                        }
                    }
                    DetailRow("Attachments", details.attachments?.let(::boolLabel))
                    DetailRow("Open weights", details.openWeights?.let(::boolLabel))
                    DetailRow("Temperature control", details.temperatureControl?.let(::boolLabel))
                    DetailRow("Pricing source", details.pricing?.source)
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
            fontSize = 12.sp,
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

private fun boolLabel(value: Boolean): String = if (value) "Yes" else "No"
