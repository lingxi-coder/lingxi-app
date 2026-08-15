import SwiftUI

/// Compact provider-aware controls surface. The engine supplies the option
/// identifiers; this view only renders them and sends the selected identifier
/// back, so unsupported provider/model combinations cannot be invented here.
struct ConversationControlsSheet: View {
    @Environment(\.dismiss) private var dismiss
    let reasoningSelection: String
    let reasoningOptions: [String]
    let reasoningOptionDetails: [ConversationReasoningOption]
    let reasoningBudgetRange: ClosedRange<UInt64>?
    let reasoningDisabledReason: String?
    let permissionMode: String
    let effectivePermissionMode: String
    let permissionOptions: [ConversationPermissionOption]
    let controlsPending: Bool
    let controlsError: String?
    let onSelectReasoning: (String) -> Void
    let onSelectPermission: (String) -> Void
    let onDismiss: () -> Void

    @State private var budgetText = ""
    @State private var pendingRiskMode: String?

    var body: some View {
        NavigationStack {
            List {
                if let controlsError {
                    Label(controlsError, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("composer.controls.error")
                }
                Section("Reasoning") {
                    if reasoningOptionDetails.isEmpty && reasoningOptions.isEmpty && reasoningBudgetRange == nil {
                        Text("Automatic (provider default)").foregroundStyle(.secondary)
                    } else {
                        if let reasoningDisabledReason {
                            Label(reasoningDisabledReason, systemImage: "info.circle")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                                .accessibilityIdentifier("composer.reasoning.disabled-reason")
                        }
                        ForEach(nonBudgetOptions) { option in
                            Button {
                                onSelectReasoning(option.id)
                            } label: {
                                HStack {
                                    Text(option.title)
                                    Spacer()
                                    if option.id == reasoningSelection { Image(systemName: "checkmark") }
                                }
                            }
                            .accessibilityIdentifier("composer.reasoning.\(option.id)")
                            .disabled(controlsPending)
                        }
                        if let reasoningBudgetRange {
                            budgetEditor(range: reasoningBudgetRange)
                        }
                    }
                }
                Section("Permission") {
                    ForEach(permissionOptions) { option in
                        Button {
                            if option.id == "dontAsk" || option.id == "bypassPermissions" {
                                pendingRiskMode = option.id
                            } else {
                                onSelectPermission(option.id)
                            }
                        } label: {
                            HStack {
                                Text(option.id)
                                Spacer()
                                if option.id == permissionMode { Image(systemName: "checkmark") }
                                if option.id == effectivePermissionMode && option.id != permissionMode {
                                    Text("effective").font(.caption).foregroundStyle(.secondary)
                                }
                                if !option.available {
                                    Image(systemName: "lock").foregroundStyle(.secondary)
                                }
                            }
                        }
                        .disabled(!option.available || controlsPending)
                        .accessibilityHint(option.disabledReason ?? "")
                        .accessibilityIdentifier("composer.permission.\(option.id)")
                    }
                }
            }
            .navigationTitle("Model · Effort & Permission")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close") { onDismiss(); dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .alert("Confirm permission mode", isPresented: Binding(
            get: { pendingRiskMode != nil },
            set: { if !$0 { pendingRiskMode = nil } }
        )) {
            Button("Cancel", role: .cancel) { pendingRiskMode = nil }
            Button("Confirm", role: .destructive) {
                if let mode = pendingRiskMode { onSelectPermission(mode) }
                pendingRiskMode = nil
            }
        } message: {
            Text("This mode can allow higher-risk operations without a prompt.")
        }
    }

    private var nonBudgetOptions: [ConversationReasoningOption] {
        let options = reasoningOptionDetails.isEmpty
            ? reasoningOptions.map {
                ConversationReasoningOption(
                    id: $0,
                    title: $0.capitalized,
                    isBudget: $0.hasPrefix("budget:"),
                    persistable: true
                )
            }
            : reasoningOptionDetails
        return options.filter { !$0.isBudget }
    }

    @ViewBuilder
    private func budgetEditor(range: ClosedRange<UInt64>) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("Token budget")
                Text("\(range.lowerBound)–\(range.upperBound) tokens")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            TextField("tokens", text: $budgetText)
                .keyboardType(.numberPad)
                .multilineTextAlignment(.trailing)
                .frame(width: 96)
            Button {
                guard let tokens = UInt64(budgetText), range.contains(tokens) else { return }
                onSelectReasoning("budget:\(tokens)")
            } label: {
                Image(systemName: "checkmark.circle")
            }
            .disabled(controlsPending || !validBudget)
        }
        .accessibilityIdentifier("composer.reasoning.token-budget")
        .disabled(controlsPending)
    }

    private var validBudget: Bool {
        guard let range = reasoningBudgetRange,
              let tokens = UInt64(budgetText)
        else { return false }
        return range.contains(tokens)
    }
}
