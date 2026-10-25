import SwiftUI

/// Compact provider-aware controls surface. The engine supplies the option
/// identifiers; this view only renders them and sends the selected identifier
/// back, so unsupported provider/model combinations cannot be invented here.
struct ConversationControlsSheet: View {
    @Environment(\.dismiss) private var dismiss
    let permissionMode: String
    let effectivePermissionMode: String
    let permissionOptions: [ConversationPermissionOption]
    let controlsPending: Bool
    let controlsError: String?
    let bypassWarningSuppressed: Bool
    let onSelectPermission: (String) -> Void
    let onConfirmBypassPermissions: (Bool) -> Void
    let onDismiss: () -> Void

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
                Section(String(localized: "settings_parity_permissions")) {
                    ForEach(permissionOptions) { option in
                        Button {
                            guard option.id != permissionMode else { return }
                            if Self.requiresRiskConfirmation(
                                for: option.id,
                                bypassWarningSuppressed: bypassWarningSuppressed
                            ) {
                                pendingRiskMode = option.id
                            } else if option.id == "bypassPermissions" {
                                onConfirmBypassPermissions(false)
                            } else {
                                onSelectPermission(option.id)
                            }
                        } label: {
                            HStack {
                                Text(ModelDetailFormat.permissionModeLabel(option.id))
                                Spacer()
                                if option.id == permissionMode { Image(systemName: "checkmark") }
                                if option.id == effectivePermissionMode && option.id != permissionMode {
                                    Text(String(localized: "composer_permission_effective")).font(.caption).foregroundStyle(.secondary)
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
            .navigationTitle(String(localized: "settings_permission_mode_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close") { onDismiss(); dismiss() }
                        .accessibilityIdentifier("composer.permission.close")
                }
            }
        }
        .accessibilityIdentifier("composer.permission.sheet")
        .presentationDetents([.medium, .large])
        .alert(String(localized: "settings_permission_mode_confirm_title"), isPresented: Binding(
            get: { pendingRiskMode != nil },
            set: { if !$0 { pendingRiskMode = nil } }
        )) {
            Button(String(localized: "common_cancel"), role: .cancel) { pendingRiskMode = nil }
            if pendingRiskMode == "bypassPermissions" {
                Button(String(localized: "composer_enter_full_access"), role: .destructive) {
                    onConfirmBypassPermissions(false)
                    pendingRiskMode = nil
                }
                Button(String(localized: "composer_dont_warn_again"), role: .destructive) {
                    onConfirmBypassPermissions(true)
                    pendingRiskMode = nil
                }
            } else {
                Button(String(localized: "settings_permission_mode_confirm"), role: .destructive) {
                    if let mode = pendingRiskMode { onSelectPermission(mode) }
                    pendingRiskMode = nil
                }
            }
        } message: {
            if pendingRiskMode == "bypassPermissions" {
                Text(String(localized: "composer_full_access_warning"))
            } else {
                Text(String(localized: "settings_permission_mode_confirm_detail"))
            }
        }
    }

    static func requiresRiskConfirmation(
        for mode: String,
        bypassWarningSuppressed: Bool
    ) -> Bool {
        switch mode {
        case "dontAsk":
            return true
        case "bypassPermissions":
            return !bypassWarningSuppressed
        default:
            return false
        }
    }

}

/// A discrete, provider-owned effort scale. Dragging previews locally; releasing
/// commits one supported identifier to the engine.
struct ConversationEffortControl: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let selection: String
    let options: [ConversationReasoningOption]
    let budgetRange: ClosedRange<UInt64>?
    let disabledReason: String?
    let pending: Bool
    let onSelect: (String) -> Void

    @State private var previewPosition: CGFloat?
    @State private var thumbGrabOffset: CGFloat?
    @State private var budgetText = ""

    private var discreteOptions: [ConversationReasoningOption] {
        var seen = Set<String>()
        return options.filter {
            !$0.isBudget && !$0.id.isEmpty && !["automatic", "disabled"].contains($0.id) && !$0.id.hasPrefix("budget:")
                && seen.insert($0.id).inserted
        }
    }

    private var selectedIndex: Int? {
        discreteOptions.firstIndex { $0.id == selection }
    }

    private var previewIndex: Int? {
        guard let previewPosition, !discreteOptions.isEmpty else { return selectedIndex }
        return min(discreteOptions.count - 1, max(0, Int(previewPosition.rounded())))
    }

    private var currentTitle: String {
        if let previewIndex { return ModelDetailFormat.reasoningIdentifierLabel(discreteOptions[previewIndex].id, fallback: discreteOptions[previewIndex].title) }
        return ModelDetailFormat.reasoningIdentifierLabel(selection.isEmpty ? "automatic" : selection, fallback: selection.capitalized)
    }

    private var isDisabled: Bool { pending || disabledReason != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Text("composer_effort")
                Spacer(minLength: 12)
                Text(currentTitle)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.trailing)
                    .accessibilityIdentifier("composer.reasoning.current")
            }
            if !discreteOptions.isEmpty {
                effortTrack
            } else if budgetRange == nil {
                Text(String(localized: "composer_reasoning_default"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            if let disabledReason {
                Label(disabledReason, systemImage: "info.circle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("composer.reasoning.disabled-reason")
            }
            if let budgetRange { budgetEditor(range: budgetRange) }
        }
        .onAppear { resetPreview() }
        .onChange(of: selection) { resetPreview() }
        .onChange(of: options) { resetPreview() }
        .onChange(of: budgetRange) { resetPreview() }
        .onChange(of: pending) {
            if pending {
                previewPosition = nil
                thumbGrabOffset = nil
            }
        }
    }

    private var effortTrack: some View {
        GeometryReader { geometry in
            let radius: CGFloat = 22
            let width = max(0, geometry.size.width - radius * 2)
            let steps = max(1, discreteOptions.count - 1)
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Color(uiColor: .systemGray5))
                    .overlay { Capsule().strokeBorder(.primary.opacity(0.08), lineWidth: 1) }
                ForEach(Array(discreteOptions.enumerated()), id: \.element.id) { index, _ in
                    Circle()
                        .fill(.secondary.opacity(0.5))
                        .frame(width: 6, height: 6)
                        .position(x: radius + width * CGFloat(index) / CGFloat(steps), y: radius)
                }
                if let position = previewPosition ?? selectedIndex.map({ CGFloat($0) }) {
                    Circle()
                        .fill(.white)
                        .overlay { Circle().strokeBorder(.black.opacity(0.08), lineWidth: 1) }
                        .shadow(color: .black.opacity(0.12), radius: 2, y: 1)
                        .frame(width: 44, height: 44)
                        .offset(x: width * position / CGFloat(steps))
                }
            }
            .contentShape(Capsule())
            .gesture(DragGesture(minimumDistance: 0)
                .onChanged { value in
                    guard !isDisabled, discreteOptions.count > 1, width > 0 else { return }
                    if thumbGrabOffset == nil {
                        if let position = previewPosition ?? selectedIndex.map({ CGFloat($0) }) {
                            let center = radius + width * position / CGFloat(steps)
                            let offset = value.startLocation.x - center
                            let verticalOffset = value.startLocation.y - radius
                            let grabbedThumb = offset * offset + verticalOffset * verticalOffset <= radius * radius
                            thumbGrabOffset = grabbedThumb ? offset : 0
                        } else {
                            thumbGrabOffset = 0
                        }
                    }
                    let location = value.location.x - (thumbGrabOffset ?? 0)
                    previewPosition = min(CGFloat(steps), max(0, (location - radius) / width * CGFloat(steps)))
                }
                .onEnded { _ in
                    thumbGrabOffset = nil
                    guard !isDisabled, let index = previewIndex else {
                        previewPosition = nil
                        return
                    }
                    let identifier = discreteOptions[index].id
                    withAnimation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 1)) {
                        previewPosition = CGFloat(index)
                    }
                    if identifier != selection { onSelect(identifier) }
                })
        }
        .frame(height: 44)
        .opacity(isDisabled ? 0.5 : 1)
        .disabled(isDisabled)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text("composer_effort"))
        .accessibilityValue(currentTitle)
        .accessibilityHint(disabledReason ?? String(localized: "composer_reasoning_adjust_hint"))
        .accessibilityIdentifier("composer.reasoning.slider")
        .accessibilityAdjustableAction { direction in
            guard !isDisabled, !discreteOptions.isEmpty else { return }
            let current = previewIndex ?? 0
            let index: Int
            switch direction {
            case .increment: index = min(discreteOptions.count - 1, current + 1)
            case .decrement: index = max(0, current - 1)
            @unknown default: return
            }
            let identifier = discreteOptions[index].id
            if identifier != selection { onSelect(identifier) }
        }
    }

    private func resetPreview() {
        previewPosition = nil
        thumbGrabOffset = nil
        budgetText = selection.hasPrefix("budget:") ? String(selection.dropFirst("budget:".count)) : ""
    }

    private func budgetEditor(range: ClosedRange<UInt64>) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(String(localized: "composer_reasoning_token_budget"))
            Text(String(localized: "composer_reasoning_budget_range \(String(range.lowerBound)) \(String(range.upperBound))"))
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                TextField("tokens", text: $budgetText)
                    .keyboardType(.numberPad)
                    .accessibilityLabel(String(localized: "composer_reasoning_token_budget"))
                Button {
                    guard let tokens = UInt64(budgetText), range.contains(tokens) else { return }
                    onSelect("budget:\(tokens)")
                } label: {
                    Image(systemName: "checkmark.circle")
                }
                .accessibilityLabel(String(localized: "composer_reasoning_apply_budget"))
                .disabled(UInt64(budgetText).map { !range.contains($0) } ?? true)
            }
        }
        .accessibilityIdentifier("composer.reasoning.token-budget")
        .disabled(isDisabled)
    }
}
