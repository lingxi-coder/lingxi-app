import SwiftUI

struct LocalAppDesignerView: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    let appID: String
    @Binding var path: [LocalAppsRoute]

    @State private var confirming = false

    private var app: LocalAppSummary? { store.app(id: appID) }
    private var template: LocalAppTemplate? { app.flatMap(store.template) }
    private var steps: [LocalAppDesignStep] { template?.orderedSteps ?? [] }
    private var designer: LocalAppDesignerSession? { store.designers[appID] }
    private var stepIndex: Int {
        min(max(designer?.currentStep ?? 0, 0), max(steps.count - 1, 0))
    }
    private var currentStep: LocalAppDesignStep? {
        guard steps.indices.contains(stepIndex) else { return nil }
        return steps[stepIndex]
    }

    var body: some View {
        Group {
            if let app, let template, let currentStep {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 18) {
                        DesignerStepHeader(
                            templateName: template.name,
                            steps: steps,
                            selectedIndex: stepIndex,
                            onSelect: selectStep
                        )
                        if let suggestion = store.suggestions[appID] {
                            LocalAppSuggestionPanel(
                                suggestion: suggestion,
                                onApply: { Task { await store.applySuggestion(appID: appID) } },
                                onDismiss: { Task { await store.dismissSuggestion(appID: appID) } }
                            )
                        }
                        DesignerStepCard(
                            step: currentStep,
                            appID: appID,
                            values: designer?.fields ?? [:],
                            onEdit: handleEdit
                        )
                        if let progress = store.generationProgress[appID] {
                            LocalAppGenerationCard(progress: progress, workflow: app.workflow)
                        }
                    }
                    .padding()
                }
                .background(theme.windowBg)
                .safeAreaInset(edge: .bottom) {
                    DesignerBottomBar(
                        stepIndex: stepIndex,
                        stepCount: steps.count,
                        canContinue: canAdvance,
                        isConfirming: confirming,
                        onPrevious: previous,
                        onNext: next
                    )
                }
            } else {
                ContentUnavailableView {
                    Label("local_apps_designer_not_ready", systemImage: "slider.horizontal.3")
                } description: {
                    Text(template == nil ? String(localized: "local_apps_designer_waiting") : String(localized: "local_apps_designer_opening"))
                } actions: {
                    Button("common_retry") { Task { await prepare() } }
                }
            }
        }
        .navigationTitle(app?.name ?? String(localized: "local_apps_designer_title"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("local_apps_agent_suggestion", systemImage: "sparkles") {
                    Task { await store.requestDesignSuggestion(appID: appID) }
                }
                .disabled(designer == nil)
                .accessibilityIdentifier("local-apps.request-suggestion")
            }
        }
        .task { await prepare() }
    }

    private var values: [String: LocalAppDesignValue] { designer?.fields ?? [:] }

    private var canContinue: Bool {
        guard let currentStep else { return false }
        return LocalAppDesignerGate.isSatisfied(currentStep, values: values)
    }

    /// The step pills are ungated forward jumps and `confirm_design` has no
    /// required-field check of its own, so the terminal action re-checks every step.
    private var canConfirm: Bool {
        LocalAppDesignerGate.canConfirm(steps, values: values)
    }

    private var canAdvance: Bool {
        stepIndex + 1 < steps.count ? canContinue : canConfirm
    }

    private func prepare() async {
        guard store.designers[appID]?.interactionID == nil else { return }
        // `open_designer` is only legal from collecting_spec / generation_failed.
        // Once the gate is armed the app sits in awaiting_spec_confirmation and
        // the engine rejects it outright — which is what a relaunch used to
        // produce, because the client had no interaction_id yet and asked for a
        // gate that was already open. Refresh instead; the id arrives with the
        // re-announced gate (AppService::resync_pending_gates).
        if store.apps.first(where: { $0.id == appID })?.workflow == .awaitingSpecConfirmation {
            await store.getDetails(appID: appID)
            return
        }
        await store.openDesigner(appID: appID)
    }

    private func selectStep(_ index: Int) {
        store.setCurrentStep(index, appID: appID)
    }

    private func previous() {
        store.setCurrentStep(stepIndex - 1, appID: appID)
    }

    private func next() {
        guard canAdvance else { return }
        if stepIndex + 1 < steps.count {
            store.setCurrentStep(stepIndex + 1, appID: appID)
        } else {
            Task { await confirm() }
        }
    }

    private func confirm() async {
        confirming = true
        let succeeded = await store.confirmDesign(appID: appID)
        confirming = false
        if succeeded {
            path = [.details(appID)]
        }
    }

    private func handleEdit(_ field: LocalAppDesignField, _ value: LocalAppDesignValue) {
        store.edit(field: field, value: value, appID: appID)
    }
}

/// Shared by the per-step gate and the confirm gate so a forward jump cannot
/// submit a draft whose earlier steps are still empty.
enum LocalAppDesignerGate {
    /// The terminal gate: EVERY step, not just the visible one. Owned here
    /// rather than by the view so it is the same code the tests drive.
    static func canConfirm(_ steps: [LocalAppDesignStep], values: [String: LocalAppDesignValue]) -> Bool {
        steps.allSatisfy { isSatisfied($0, values: values) }
    }

    static func isSatisfied(_ step: LocalAppDesignStep, values: [String: LocalAppDesignValue]) -> Bool {
        step.fields.allSatisfy { field in
            guard field.required else { return true }
            guard let value = values[field.id] ?? field.defaultValue else { return false }
            switch value {
            case let .text(value), let .color(value), let .density(value):
                return !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            case let .strings(values), let .domains(values):
                return !values.isEmpty
            case .boolean:
                return true
            case let .dataFields(fields):
                return !fields.isEmpty
            }
        }
    }
}

private struct DesignerStepHeader: View {
    @Environment(\.theme) private var theme
    let templateName: String
    let steps: [LocalAppDesignStep]
    let selectedIndex: Int
    let onSelect: (Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(templateName)
                .font(.caption)
                .foregroundStyle(theme.text3)
            HStack(spacing: 7) {
                ForEach(Array(steps.enumerated()), id: \.element.id) { index, step in
                    Button {
                        onSelect(index)
                    } label: {
                        VStack(spacing: 5) {
                            Text("\(index + 1)")
                                .font(.caption.bold())
                                .frame(width: 28, height: 28)
                                .background(
                                    index == selectedIndex ? theme.accent : theme.surface,
                                    in: .circle
                                )
                                .foregroundStyle(index == selectedIndex ? .white : theme.text3)
                            Text(step.title)
                                .font(.caption2)
                                .foregroundStyle(index == selectedIndex ? theme.text : theme.text4)
                                .lineLimit(1)
                        }
                        .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("local_apps_designer_step \(index + 1) \(step.title)")
                    .accessibilityValue(index == selectedIndex ? "local_apps_current_step" : "")
                }
            }
        }
    }
}

private struct DesignerStepCard: View {
    @Environment(\.theme) private var theme
    let step: LocalAppDesignStep
    let appID: String
    let values: [String: LocalAppDesignValue]
    let onEdit: (LocalAppDesignField, LocalAppDesignValue) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 5) {
                Text(step.title)
                    .font(.title3.bold())
                    .foregroundStyle(theme.text)
                if !step.description.isEmpty {
                    Text(step.description)
                        .font(.subheadline)
                        .foregroundStyle(theme.text3)
                }
            }
            ForEach(step.fields) { field in
                LocalAppFieldEditor(
                    field: field,
                    value: values[field.id] ?? field.defaultValue,
                    onChange: { onEdit(field, $0) }
                )
            }
        }
        .padding(18)
        .background(theme.surface, in: .rect(cornerRadius: 18))
        .overlay {
            RoundedRectangle(cornerRadius: 18)
                .stroke(theme.border, lineWidth: 0.5)
        }
    }
}

private struct LocalAppFieldEditor: View {
    @Environment(\.theme) private var theme
    let field: LocalAppDesignField
    let value: LocalAppDesignValue?
    let onChange: (LocalAppDesignValue) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 3) {
                Text(field.label)
                    .font(.subheadline.bold())
                if field.required {
                    Text("*").foregroundStyle(theme.danger)
                }
            }
            if !field.description.isEmpty {
                Text(field.description)
                    .font(.caption)
                    .foregroundStyle(theme.text3)
            }
            editor
        }
    }

    @ViewBuilder
    private var editor: some View {
        switch field.type {
        case .shortText:
            TextField(field.label, text: textBinding)
                .textFieldStyle(.roundedBorder)
        case .longText:
            TextEditor(text: textBinding)
                .frame(minHeight: 100)
                .padding(6)
                .background(theme.windowBg, in: .rect(cornerRadius: 10))
                .overlay { RoundedRectangle(cornerRadius: 10).stroke(theme.border) }
        case .singleChoice:
            Picker(field.label, selection: textBinding) {
                Text("local_apps_please_select").tag("")
                ForEach(field.options) { option in
                    Text(option.label).tag(option.value)
                }
            }
            .pickerStyle(.menu)
            .frame(maxWidth: .infinity, alignment: .leading)
        case .multipleChoice:
            LocalAppMultipleChoiceEditor(options: field.options, selection: stringsBinding)
        case .boolean:
            Toggle(field.label, isOn: booleanBinding)
                .labelsHidden()
        case .color:
            HStack {
                TextField("#RRGGBB", text: textBinding)
                    .textFieldStyle(.roundedBorder)
                Circle()
                    .fill(colorValue)
                    .frame(width: 28, height: 28)
                    .overlay { Circle().stroke(theme.border) }
            }
        case .density:
            Picker(field.label, selection: textBinding) {
                Text("settings_density_compact").tag("compact")
                Text("settings_density_comfortable").tag("comfortable")
            }
            .pickerStyle(.segmented)
        case .screenList, .featureList:
            LocalAppStringListEditor(values: stringsBinding, placeholder: String(localized: "local_apps_line_per_item"))
        case .domainList:
            LocalAppStringListEditor(
                values: domainsBinding,
                placeholder: "api.example.com",
                normalize: LocalAppDomainPolicy.normalize
            )
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
        case .dataFieldList:
            LocalAppDataFieldsEditor(fields: dataFieldsBinding)
        }
    }

    private var textBinding: Binding<String> {
        Binding(
            get: {
                switch value {
                case let .text(value), let .color(value), let .density(value): value
                default: ""
                }
            },
            set: { newValue in
                switch field.type {
                case .color: onChange(.color(newValue))
                case .density: onChange(.density(newValue))
                default: onChange(.text(newValue))
                }
            }
        )
    }

    private var stringsBinding: Binding<[String]> {
        Binding(
            get: {
                guard case let .strings(values) = value else { return [] }
                return values
            },
            set: { onChange(.strings($0)) }
        )
    }

    private var domainsBinding: Binding<[String]> {
        Binding(
            get: {
                guard case let .domains(values) = value else { return [] }
                return values
            },
            set: { onChange(.domains($0)) }
        )
    }

    private var booleanBinding: Binding<Bool> {
        Binding(
            get: {
                guard case let .boolean(value) = value else { return false }
                return value
            },
            set: { onChange(.boolean($0)) }
        )
    }

    private var dataFieldsBinding: Binding<[LocalAppDataField]> {
        Binding(
            get: {
                guard case let .dataFields(fields) = value else { return [] }
                return fields
            },
            set: { onChange(.dataFields($0)) }
        )
    }

    private var colorValue: Color {
        guard case let .color(hex) = value else { return theme.accent }
        return Color(localAppHex: hex) ?? theme.accent
    }
}

private struct LocalAppMultipleChoiceEditor: View {
    let options: [LocalAppDesignOption]
    @Binding var selection: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(options) { option in
                Toggle(
                    option.label,
                    isOn: Binding(
                        get: { selection.contains(option.value) },
                        set: { enabled in
                            if enabled, !selection.contains(option.value) { selection.append(option.value) }
                            if !enabled { selection.removeAll { $0 == option.value } }
                        }
                    )
                )
            }
        }
    }
}

private struct LocalAppStringListEditor: View {
    @Binding var values: [String]
    let placeholder: String
    /// Applied to every line before it becomes a value; `nil` drops the line.
    let normalize: (String) -> String?

    @State private var text: String

    init(
        values: Binding<[String]>,
        placeholder: String,
        normalize: @escaping (String) -> String? = { $0 }
    ) {
        _values = values
        self.placeholder = placeholder
        self.normalize = normalize
        _text = State(initialValue: values.wrappedValue.joined(separator: "\n"))
    }

    var body: some View {
        TextEditor(text: $text)
        .frame(minHeight: 90)
        .overlay(alignment: .topLeading) {
            if text.isEmpty {
                Text(placeholder)
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 8)
                    .allowsHitTesting(false)
            }
        }
        .onChange(of: text) { _, newValue in
            let parsed = parse(newValue)
            if parsed != values { values = parsed }
        }
        .onChange(of: values) { _, newValue in
            guard parse(text) != newValue else { return }
            text = newValue.joined(separator: "\n")
        }
    }

    private func parse(_ value: String) -> [String] {
        value
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .compactMap(normalize)
            .filter { !$0.isEmpty }
    }
}

/// Mirrors the engine's `manifest::validate_domain`, which the draft gate now
/// enforces at ingest, and additionally strips what a pasted URL carries. A
/// half-typed or pasted line is therefore dropped locally rather than raising
/// the raw English engine `invalid_request` in the 应用错误 alert — the same
/// job Android's `isValidDomain` does before submitting.
enum LocalAppDomainPolicy {
    static func normalize(_ value: String) -> String? {
        var host = value.lowercased()
        if let scheme = host.range(of: "://") { host = String(host[scheme.upperBound...]) }
        if let path = host.firstIndex(where: { $0 == "/" || $0 == "?" || $0 == "#" }) {
            host = String(host[..<path])
        }
        if let credentials = host.lastIndex(of: "@") {
            host = String(host[host.index(after: credentials)...])
        }
        if let port = host.firstIndex(of: ":") { host = String(host[..<port]) }
        guard !host.isEmpty, host.count <= 253 else { return nil }
        let labelsAreValid = host
            .split(separator: ".", omittingEmptySubsequences: false)
            .allSatisfy { label in
                !label.isEmpty && label.count <= 63
                    && !label.hasPrefix("-") && !label.hasSuffix("-")
                    && label.allSatisfy {
                        ("a" ... "z").contains($0) || ("0" ... "9").contains($0) || $0 == "-"
                    }
            }
        return labelsAreValid ? host : nil
    }
}

private struct LocalAppDataFieldsEditor: View {
    @Binding var fields: [LocalAppDataField]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach($fields) { $field in
                HStack {
                    TextField("local_apps_field_name", text: $field.name)
                    Picker("local_apps_field_type", selection: $field.type) {
                        ForEach(LocalAppDataFieldType.allCases, id: \.rawValue) { type in
                            Text(type.rawValue).tag(type)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    Toggle("local_apps_field_required", isOn: $field.required)
                        .labelsHidden()
                    Button("local_apps_field_delete", systemImage: "minus.circle", role: .destructive) {
                        fields.removeAll { $0.id == field.id }
                    }
                    .labelStyle(.iconOnly)
                }
            }
            Button("local_apps_field_add", systemImage: "plus") {
                fields.append(
                    LocalAppDataField(
                        id: LocalAppDataFieldIDPolicy.nextID(existing: fields),
                        name: "",
                        type: .text,
                        required: false,
                        options: []
                    )
                )
            }
        }
    }
}

/// Field ids reach the app manifest verbatim, where `validate_identifier`
/// requires `^[a-z][a-z0-9_]{0,63}$` (lingxi-code/local-apps/src/manifest.rs).
/// A UUID is hyphenated and usually digit-initial, so it is rejected at ingest —
/// and before the engine gained that gate it stranded the app in GenerationFailed,
/// whose draft can no longer be edited.
enum LocalAppDataFieldIDPolicy {
    static func nextID(existing: [LocalAppDataField]) -> String {
        var index = existing.count + 1
        while existing.contains(where: { $0.id == "field_\(index)" }) {
            index += 1
        }
        return "field_\(index)"
    }
}

private struct LocalAppSuggestionPanel: View {
    @Environment(\.theme) private var theme
    let suggestion: LocalAppSuggestionDiff
    let onApply: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("local_apps_agent_suggestion", systemImage: "sparkles")
                .font(.headline)
                .foregroundStyle(theme.accent)
            Text(suggestion.summary)
                .font(.subheadline)
            ForEach(suggestion.changes) { change in
                VStack(alignment: .leading, spacing: 3) {
                    Text(change.fieldID)
                        .font(.caption.bold())
                    Text("local_apps_diff_change \(change.oldValue?.textValue ?? String(localized: "common_unset")) \(change.newValue?.textValue ?? String(localized: "common_removed"))")
                        .font(.caption)
                        .foregroundStyle(theme.text3)
                }
            }
            HStack {
                Button("local_apps_ignore", action: onDismiss)
                    .buttonStyle(.bordered)
                Button("local_apps_apply_suggestion", action: onApply)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(16)
        .background(theme.accent.opacity(0.1), in: .rect(cornerRadius: 16))
        .overlay { RoundedRectangle(cornerRadius: 16).stroke(theme.accent.opacity(0.35)) }
    }
}

private struct DesignerBottomBar: View {
    let stepIndex: Int
    let stepCount: Int
    let canContinue: Bool
    let isConfirming: Bool
    let onPrevious: () -> Void
    let onNext: () -> Void

    var body: some View {
        HStack {
            Button("local_apps_previous", systemImage: "chevron.left", action: onPrevious)
                .disabled(stepIndex == 0)
            Spacer()
            Text("\(stepIndex + 1) / \(stepCount)")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            Button(
                stepIndex + 1 == stepCount
                    ? (isConfirming ? "local_apps_confirming" : "local_apps_confirm_generate")
                    : "local_apps_next",
                action: onNext
            )
            .buttonStyle(.borderedProminent)
            .disabled(!canContinue || isConfirming)
        }
        .padding()
        .background(.bar)
    }
}

struct LocalAppGenerationCard: View {
    let progress: LocalAppGenerationProgress
    let workflow: LocalAppWorkflow

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                ProgressView()
                Text(workflow.label).bold()
                Spacer()
                if let percent = progress.percent { Text("\(percent)%") }
            }
            if let percent = progress.percent {
                ProgressView(value: Double(percent), total: 100)
            }
            Text(progress.detail ?? progress.stage)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding()
        .background(.quaternary, in: .rect(cornerRadius: 14))
    }
}

private extension Color {
    init?(localAppHex: String) {
        let value = localAppHex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        guard value.count == 6, let number = UInt64(value, radix: 16) else { return nil }
        self.init(
            .sRGB,
            red: Double((number >> 16) & 0xff) / 255,
            green: Double((number >> 8) & 0xff) / 255,
            blue: Double(number & 0xff) / 255,
            opacity: 1
        )
    }
}
