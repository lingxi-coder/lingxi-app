import SwiftUI

struct ModelDetailsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.theme) private var t

    let details: ModelRuntimeDetails
    let accent: Color

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(details.preferredDisplayName ?? ModelDisplay.modelName(for: details.modelId))
                            .font(.system(size: 20, weight: .semibold))
                            .foregroundStyle(t.text)
                        if let description = details.description {
                            Text(description)
                                .font(.system(size: 13))
                                .foregroundStyle(t.text3)
                        }
                        Text("\((details.preferredProviderLabel ?? ModelDisplay.providerName(for: details.providerId))) · \(details.modelId)")
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(t.text3)
                            .textSelection(.enabled)
                    }
                    .padding(.vertical, 2)
                }

                detailSection(String(localized: "model_detail_basic"), rows: [
                    (String(localized: "model_detail_route"), details.reference),
                    (String(localized: "model_detail_provider"), details.providerId),
                    (String(localized: "model_detail_family"), details.family),
                    (String(localized: "session_details_status"), details.status),
                    (String(localized: "model_detail_release_date"), details.releaseDate),
                    (String(localized: "model_detail_updated"), details.lastUpdated),
                    (String(localized: "model_detail_knowledge_cutoff"), details.knowledgeCutoff),
                ])

                detailSection(String(localized: "model_detail_capabilities"), rows: [
                    (String(localized: "model_detail_input_modalities"), list(details.inputModalities)),
                    (String(localized: "model_detail_output_modalities"), list(details.outputModalities)),
                    (String(localized: "model_detail_streaming"), bool(details.capabilities.streaming)),
                    (String(localized: "apps_category_tools"), bool(details.capabilities.tools)),
                    (String(localized: "model_detail_vision"), bool(details.capabilities.vision)),
                    (String(localized: "model_detail_documents"), bool(details.capabilities.documents)),
                    (String(localized: "model_detail_reasoning"), bool(details.capabilities.reasoning)),
                    (String(localized: "model_detail_structured_output"), bool(details.capabilities.structuredOutput)),
                    (String(localized: "model_detail_open_weights"), ModelDetailFormat.yesNo(details.openWeights)),
                    (String(localized: "model_detail_attachments"), ModelDetailFormat.yesNo(details.attachments)),
                    (String(localized: "model_detail_temperature"), ModelDetailFormat.yesNo(details.temperatureControl)),
                ])

                detailSection(String(localized: "model_detail_limits"), rows: [
                    (String(localized: "model_detail_max_context"), token(details.contextWindowTokens)),
                    (String(localized: "model_detail_input_budget"), token(details.contextBudget?.effectiveInputBudget)),
                    (String(localized: "model_detail_context_policy"), details.contextBudget?.policy),
                    (String(localized: "model_detail_standard_price_limit"), details.contextBudget?.standardPriceInputTokens?.formatted()),
                    (String(localized: "model_detail_recommended_input"), token(details.contextBudget?.recommendationInputTokens)),
                    (String(localized: "model_detail_recommendation_source"), details.contextBudget?.recommendationSource),
                    (String(localized: "model_detail_max_input"), token(details.maxInputTokens)),
                    (String(localized: "model_detail_max_output"), token(details.maxOutputTokens)),
                ])

                reasoningSection
                pricingSection
            }
            .listStyle(.insetGrouped)
            .navigationTitle("composer_model_details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close") { dismiss() }
                        .tint(accent)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    @ViewBuilder
    private var reasoningSection: some View {
        Section(String(localized: "model_detail_reasoning")) {
            detailRow(String(localized: "settings_badge_default"), ModelDetailFormat.reasoningSelectionLabel(details.reasoning.providerDefault))
            detailRow(String(localized: "model_detail_editable"), bool(details.reasoning.editable))
            detailRow(String(localized: "model_detail_forced"), bool(details.reasoning.forcedReasoning))
            if let range = details.reasoning.budgetRange {
                detailRow(
                    String(localized: "model_detail_budget_range"),
                    "\(ModelDetailFormat.tokenCount(range.minTokens)) – \(ModelDetailFormat.tokenCount(range.maxTokens))"
                )
            }
            if !details.reasoning.options.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text(String(localized: "model_detail_options"))
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(t.text3)
                    ForEach(Array(details.reasoning.options.enumerated()), id: \.offset) { _, option in
                        Text(
                            option.persistable
                                ? ModelDetailFormat.reasoningSelectionLabel(option.selection)
                                : String(localized: "model_detail_session_only \(ModelDetailFormat.reasoningSelectionLabel(option.selection))")
                        )
                        .font(.system(size: 13))
                        .foregroundStyle(t.text)
                    }
                }
                .padding(.vertical, 2)
            }
            if let disabled = details.reasoning.disabledReason {
                detailRow(String(localized: "model_detail_disabled_reason"), disabled.message ?? disabled.code)
            }
        }
    }

    @ViewBuilder
    private var pricingSection: some View {
        Section(String(localized: "model_detail_pricing")) {
            if let pricing = details.pricing {
                detailRow(String(localized: "model_detail_billing"), ModelDetailFormat.billingModeLabel(pricing.billingMode))
                detailRow(String(localized: "chat_usage_input"), money(pricing.inputPerMillion))
                detailRow(String(localized: "chat_tool_verb_output"), money(pricing.outputPerMillion))
                detailRow(String(localized: "model_detail_cache_read"), money(pricing.cacheReadPerMillion))
                detailRow(String(localized: "chat_usage_cache_write"), money(pricing.cacheWritePerMillion))
                detailRow(String(localized: "model_detail_reasoning"), money(pricing.reasoningPerMillion))
                detailRow(String(localized: "audio_source_title"), pricing.source)
                if !pricing.tiers.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(String(localized: "model_detail_pricing_tiers"))
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(t.text3)
                        ForEach(Array(pricing.tiers.enumerated()), id: \.offset) { _, tier in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(String(localized: "model_detail_price_range \(tier.contextThresholdTokens.formatted()) \(tier.maxInputTokens.map { $0.formatted() } ?? "∞")"))
                                    .font(.system(size: 13, weight: .medium))
                                    .foregroundStyle(t.text)
                                Text(
                                    [
                                        tier.inputPerMillion.map { String(localized: "model_detail_price_input \(ModelDetailFormat.price($0))") },
                                        tier.outputPerMillion.map { String(localized: "model_detail_price_output \(ModelDetailFormat.price($0))") },
                                        tier.cacheReadPerMillion.map { String(localized: "model_detail_price_cache_read \(ModelDetailFormat.price($0))") },
                                        tier.cacheWritePerMillion.map { String(localized: "model_detail_price_cache_write \(ModelDetailFormat.price($0))") },
                                        tier.reasoningPerMillion.map { String(localized: "model_detail_price_reasoning \(ModelDetailFormat.price($0))") },
                                    ]
                                        .compactMap { $0 }
                                        .joined(separator: " · ")
                                )
                                .font(.system(size: 12))
                                .foregroundStyle(t.text3)
                            }
                            .padding(.vertical, 2)
                        }
                    }
                }
            } else {
                Text(String(localized: "model_detail_no_price"))
                    .font(.system(size: 13))
                    .foregroundStyle(t.text3)
            }
        }
    }

    @ViewBuilder
    private func detailSection(_ title: String, rows: [(String, String?)]) -> some View {
        let presentRows = rows.filter { value($0.1) != nil }
        if !presentRows.isEmpty {
            Section(title) {
                ForEach(Array(presentRows.enumerated()), id: \.offset) { _, row in
                    detailRow(row.0, row.1)
                }
            }
        }
    }

    @ViewBuilder
    private func detailRow(_ label: String, _ rawValue: String?) -> some View {
        if let value = value(rawValue) {
            LabeledContent(label) {
                Text(value)
                    .font(.system(size: 13))
                    .foregroundStyle(t.text2)
                    .multilineTextAlignment(.trailing)
                    .textSelection(.enabled)
            }
        }
    }

    private func token(_ value: UInt64?) -> String? {
        value.map(ModelDetailFormat.tokenCount)
    }

    private func money(_ value: Double?) -> String? {
        value.map { "$\(ModelDetailFormat.price($0)) / 1M" }
    }

    private func bool(_ value: Bool) -> String {
        value ? String(localized: "common_yes") : String(localized: "common_no")
    }

    private func list(_ values: [String]) -> String? {
        value(values.joined(separator: ", "))
    }

    private func value(_ raw: String?) -> String? {
        guard let raw, !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return nil
        }
        return raw
    }
}
