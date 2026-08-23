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

                detailSection("基本信息", rows: [
                    ("路由", details.reference),
                    ("Provider", details.providerId),
                    ("Family", details.family),
                    ("状态", details.status),
                    ("发布日期", details.releaseDate),
                    ("最近更新", details.lastUpdated),
                    ("知识截止", details.knowledgeCutoff),
                ])

                detailSection("模态与能力", rows: [
                    ("输入模态", list(details.inputModalities)),
                    ("输出模态", list(details.outputModalities)),
                    ("Streaming", bool(details.capabilities.streaming)),
                    ("Tools", bool(details.capabilities.tools)),
                    ("Vision", bool(details.capabilities.vision)),
                    ("Documents", bool(details.capabilities.documents)),
                    ("Reasoning", bool(details.capabilities.reasoning)),
                    ("Structured Output", bool(details.capabilities.structuredOutput)),
                    ("Open Weights", ModelDetailFormat.yesNo(details.openWeights)),
                    ("Attachments", ModelDetailFormat.yesNo(details.attachments)),
                    ("Temperature Control", ModelDetailFormat.yesNo(details.temperatureControl)),
                ])

                detailSection("Limits", rows: [
                    ("Context Window", token(details.contextWindowTokens)),
                    ("Max Input", token(details.maxInputTokens)),
                    ("Max Output", token(details.maxOutputTokens)),
                ])

                reasoningSection
                pricingSection
            }
            .listStyle(.insetGrouped)
            .navigationTitle("模型详情")
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
        Section("Reasoning") {
            detailRow("默认", ModelDetailFormat.reasoningSelectionLabel(details.reasoning.providerDefault))
            detailRow("可编辑", bool(details.reasoning.editable))
            detailRow("强制开启", bool(details.reasoning.forcedReasoning))
            if let range = details.reasoning.budgetRange {
                detailRow(
                    "Budget Range",
                    "\(ModelDetailFormat.tokenCount(range.minTokens)) – \(ModelDetailFormat.tokenCount(range.maxTokens))"
                )
            }
            if !details.reasoning.options.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("可选项")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(t.text3)
                    ForEach(Array(details.reasoning.options.enumerated()), id: \.offset) { _, option in
                        Text(
                            option.persistable
                                ? ModelDetailFormat.reasoningSelectionLabel(option.selection)
                                : "\(ModelDetailFormat.reasoningSelectionLabel(option.selection))（仅当前会话）"
                        )
                        .font(.system(size: 13))
                        .foregroundStyle(t.text)
                    }
                }
                .padding(.vertical, 2)
            }
            if let disabled = details.reasoning.disabledReason {
                detailRow("不可编辑原因", disabled.message ?? disabled.code)
            }
        }
    }

    @ViewBuilder
    private var pricingSection: some View {
        Section("Pricing") {
            if let pricing = details.pricing {
                detailRow("计费方式", ModelDetailFormat.billingModeLabel(pricing.billingMode))
                detailRow("Input", money(pricing.inputPerMillion))
                detailRow("Output", money(pricing.outputPerMillion))
                detailRow("Cache Read", money(pricing.cacheReadPerMillion))
                detailRow("Cache Write", money(pricing.cacheWritePerMillion))
                detailRow("Reasoning", money(pricing.reasoningPerMillion))
                detailRow("来源", pricing.source)
                if !pricing.tiers.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Pricing Tiers")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(t.text3)
                        ForEach(Array(pricing.tiers.enumerated()), id: \.offset) { _, tier in
                            VStack(alignment: .leading, spacing: 4) {
                                Text("≥ \(ModelDetailFormat.tokenCount(tier.contextThresholdTokens))")
                                    .font(.system(size: 13, weight: .medium))
                                    .foregroundStyle(t.text)
                                Text(
                                    [
                                        tier.inputPerMillion.map { "in $\(ModelDetailFormat.price($0))" },
                                        tier.outputPerMillion.map { "out $\(ModelDetailFormat.price($0))" },
                                        tier.cacheReadPerMillion.map { "cache read $\(ModelDetailFormat.price($0))" },
                                        tier.cacheWritePerMillion.map { "cache write $\(ModelDetailFormat.price($0))" },
                                        tier.reasoningPerMillion.map { "reasoning $\(ModelDetailFormat.price($0))" },
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
                Text("价格未提供")
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
        value ? "是" : "否"
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
