import SwiftUI

struct LocalAppDesignerView: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    let appID: String
    @Binding var path: [LocalAppsRoute]

    @State private var confirming = false
    /// Guards the two retry buttons in `unavailableView(for:)` against a
    /// double-tap firing two `retry_questionnaire`/`retry_plan` commands
    /// before the workflow transition lands and the button disappears.
    /// `ensure_workflow` (state.rs) rejects the second server-side, so a
    /// double-tap cannot corrupt anything — but each one is a real LLM round
    /// trip, so the wasted one is worth guarding against client-side too.
    @State private var retryingQuestionnaire = false
    @State private var retryingPlan = false
    /// Guards Task 15's plan-confirmation sheet's two buttons against a
    /// double-tap firing two `confirm_design`/`cancel_design` commands
    /// before the workflow transition lands and `planConfirmBinding`'s
    /// getter (driven by `app.workflow`/`store.plans[appID]`) flips the
    /// sheet closed. Same reasoning as `retryingQuestionnaire`/`retryingPlan`
    /// above: the engine rejects a redundant second command harmlessly, but
    /// each one is a real round trip worth not wasting.
    @State private var planSheetBusy = false

    private var app: LocalAppSummary? { store.app(id: appID) }
    /// The LLM-authored questionnaire (local-apps#questionnaire, Task 13),
    /// replacing the deleted static `LocalAppTemplate.orderedSteps`.
    private var steps: [LocalAppDesignStep] { store.questionnaires[appID] ?? [] }
    private var designer: LocalAppDesignerSession? { store.designers[appID] }
    private var stepIndex: Int {
        min(max(designer?.currentStep ?? 0, 0), max(steps.count - 1, 0))
    }
    private var currentStep: LocalAppDesignStep? {
        guard steps.indices.contains(stepIndex) else { return nil }
        return steps[stepIndex]
    }

    /// Whether the answering form is interactive. `false` for every state
    /// but `collectingSpec` — in particular for `authoringQuestionnaire` and
    /// `planning`, where an LLM round trip owns the draft and a concurrent
    /// user edit would race it (local-apps#questionnaire, Task 14). A pure
    /// function of workflow so it is directly testable without a live view.
    static func isEditable(_ workflow: LocalAppWorkflow) -> Bool {
        workflow == .collectingSpec
    }

    private var isFormEditable: Bool {
        app.map { LocalAppDesignerView.isEditable($0.workflow) } ?? false
    }

    var body: some View {
        Group {
            if let app {
                if isFormEditable, let currentStep {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 18) {
                            DesignerStepHeader(
                                appName: app.name,
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
                    unavailableView(for: app.workflow)
                }
            } else {
                ContentUnavailableView {
                    Label("local_apps_designer_not_ready", systemImage: "slider.horizontal.3")
                } description: {
                    Text(steps.isEmpty ? String(localized: "local_apps_designer_waiting") : String(localized: "local_apps_designer_opening"))
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
                .disabled(designer == nil || !isFormEditable)
                .accessibilityIdentifier("local-apps.request-suggestion")
            }
        }
        // Keyed on workflow (not just `.task { }`, which only runs once on
        // appear): a freshly created app is in `authoring_questionnaire`,
        // where `prepare()` below deliberately does nothing. `.task(id:)`
        // re-invokes `prepare()` the moment `questionnaire_ready` flips this
        // app to `.collectingSpec` — no user action required to pick the
        // retry back up.
        .task(id: app?.workflow) { await prepare() }
        // Task 15's plan-confirmation gate — the human confirmation that
        // makes `store.confirmDesign`/`cancelDesign` reachable for the first
        // time anywhere in `Sources/LocalApps/`. An independent sheet, NOT a
        // step of the questionnaire above: it renders its own
        // `NavigationStack` and carries no step-bar "previous/next" editing
        // semantics. `planConfirmBinding`'s getter is entirely
        // store-state-derived (workflow + a non-nil plan); the
        // `.interactiveDismissDisabled()` below is what makes that safe —
        // otherwise a swipe-to-dismiss would hide the sheet while leaving
        // the app parked in `awaiting_spec_confirmation` with no plan and no
        // editable form behind it, the same "reachable but dead-ended" shape
        // Task 14's review caught for `.generationFailed`. The two buttons
        // inside the sheet (`onBack` -> `cancel_design`, `onConfirm` ->
        // `confirm_design`) are therefore the ONLY way this sheet closes.
        .sheet(isPresented: planConfirmBinding) {
            if let plan = store.plans[appID] {
                LocalAppPlanConfirmView(
                    plan: plan,
                    onConfirm: { Task { await confirmPlan() } },
                    onBack: { Task { await cancelPlan() } },
                    isConfirming: planSheetBusy
                )
                .interactiveDismissDisabled()
            }
        }
    }

    /// `true` exactly when there is a plan to confirm: `awaiting_spec_
    /// confirmation` with a non-nil `store.plans[appID]`. Mirrors
    /// `confirm_design`'s own server-side freshness check
    /// (`plan_for_revision == revision`, state.rs) on the client — an
    /// answer edit voids the plan server-side and announces `plan: nil` via
    /// `appPlanChanged`, which clears `store.plans[appID]` and, through this
    /// binding, closes the sheet before it could ever offer to confirm a
    /// plan the server already discarded.
    private var planConfirmBinding: Binding<Bool> {
        Binding(
            get: { app?.workflow == .awaitingSpecConfirmation && store.plans[appID] != nil },
            set: { _ in }
        )
    }

    private func confirmPlan() async {
        guard !planSheetBusy else { return }
        planSheetBusy = true
        _ = await store.confirmDesign(appID: appID)
        planSheetBusy = false
    }

    private func cancelPlan() async {
        guard !planSheetBusy else { return }
        planSheetBusy = true
        await store.cancelDesign(appID: appID)
        planSheetBusy = false
    }

    /// The intermediate/failure states this screen renders instead of the
    /// editable form, plus a fallback for everything else the designer can
    /// transiently be pushed onto. `.generating`/`.validating`/
    /// `.awaitingPreviewConfirmation` are the sequence Task 15's plan
    /// confirmation sheet (`onConfirm` -> `confirmDesign`) hands off into:
    /// closing that sheet does not navigate `path` (see its doc comment),
    /// so this screen — still `.designer(appID)` on the stack — is the only
    /// thing on screen through the whole round trip. Before this fix all
    /// three fell into the generic `default:` below, which shows a
    /// deliberately no-op "重试" (review F2: a confirmed design stranded the
    /// user on a dead "设计器未就绪" screen with no progress and no way to
    /// the preview gate). `awaitingSpecConfirmation` is
    /// normally owned by Task 15's `LocalAppPlanConfirmView` sheet
    /// (presented from `body` whenever `store.plans[appID] != nil`) rather
    /// than by this case — this case's own copy only shows once the plan is
    /// actually gone (or briefly, before it has first loaded), and gives a
    /// real way out via `cancelDesign` rather than the generic `default:`
    /// fallback's inert "重试". `generationFailed` is likewise transient as
    /// of Task 15: `prepare()`'s `.generationFailed` case calls
    /// `openDesigner`, which moves the workflow to `awaitingSpecConfirmation`
    /// (and this screen along with it) the instant the gate lands. Every
    /// state that lands in the `default:` fallback below does so because
    /// `prepare()` never issues a doomed command, or none at all, for it —
    /// see `prepare()`.
    @ViewBuilder
    private func unavailableView(for workflow: LocalAppWorkflow) -> some View {
        switch workflow {
        case .authoringQuestionnaire:
            ContentUnavailableView {
                ProgressView()
            } description: {
                Text("local_apps_authoring_questionnaire")
            }
        case .questionnaireFailed:
            ContentUnavailableView {
                Label("local_apps_workflow_questionnaire_failed", systemImage: "exclamationmark.triangle")
            } description: {
                Text("local_apps_questionnaire_failed_detail")
            } actions: {
                VStack(spacing: 12) {
                    Button("common_retry") {
                        Task {
                            retryingQuestionnaire = true
                            await store.retryQuestionnaire(appID: appID)
                            retryingQuestionnaire = false
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(retryingQuestionnaire)
                    DesignerBriefEditor(store: store, appID: appID, brief: app?.brief ?? "")
                }
            }
        case .planning:
            ContentUnavailableView {
                ProgressView()
            } description: {
                Text("local_apps_planning")
            }
        case .planFailed:
            ContentUnavailableView {
                Label("local_apps_workflow_plan_failed", systemImage: "exclamationmark.triangle")
            } description: {
                Text("local_apps_plan_failed_detail")
            } actions: {
                Button("common_retry") {
                    Task {
                        retryingPlan = true
                        await store.retryPlan(appID: appID)
                        retryingPlan = false
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(retryingPlan)
            }
        case .awaitingSpecConfirmation:
            // Reached only while `store.plans[appID] == nil` — with a plan,
            // `planConfirmBinding` already presents the real sheet over this.
            // `AwaitingSpecConfirmation` IS in `DRAFT_EDITABLE_STATES`
            // (state.rs), so `update_draft`/`apply_suggestion` (an
            // agent-proposed revision, or a queued edit whose rejection during
            // `planning` races the transition) can null `draft.plan` without
            // moving the workflow off this state — and once that happens,
            // nothing else on screen offers a way out: `prepare()`'s own
            // `.awaitingSpecConfirmation` arm is unreachable once
            // `interactionID` is already cached, and `LocalAppDetailView`'s
            // "重试生成" is gated on `.generationFailed`, not this state
            // (review Important 2). Real copy + `cancelDesign` instead of the
            // generic fallback's inert "重试".
            ContentUnavailableView {
                Label("local_apps_plan_invalidated_title", systemImage: "exclamationmark.triangle")
            } description: {
                Text("local_apps_plan_invalidated_detail")
            } actions: {
                Button("local_apps_plan_confirm_back") { Task { await cancelPlan() } }
                    .buttonStyle(.borderedProminent)
                    .disabled(planSheetBusy)
            }
        case .generating, .validating:
            // `confirm_design`/`retry_generation` accepted and the engine is
            // now generating (or validating). This is a conversation with a
            // model, so it is rendered BY the conversation's own views
            // (`LocalAppGenerationTranscriptView` → `TranscriptScroll` +
            // `MessageBubble` + `Composer`) rather than by a spinner: the
            // stages take tens of seconds each, and a spinner cannot
            // distinguish slow from wedged.
            LocalAppGenerationTranscriptView(
                store: store,
                appID: appID,
                workflowLabel: workflow.label,
                acceptsInput: LocalAppDetailView.showsRevisionInput(for: workflow)
            )
        case .awaitingPreviewConfirmation:
            // The preview confirmation gate just armed (`appPreviewReady`,
            // now wired in `LocalAppsStore` to also set
            // `requestedPresentationAppID` the same way `appUiRequest`
            // already does). This screen is still `.designer(appID)` on
            // `path`, so route to `.preview(appID)` — the SAME destination
            // `LocalAppsLibraryScreen.open(_:)` and `LocalAppDetailView`'s
            // "打开预览" button already use — rather than the generic
            // `default:` fallback's inert retry.
            ContentUnavailableView {
                Label("local_apps_workflow_awaiting_preview", systemImage: "checkmark.seal")
            } description: {
                Text("local_apps_workflow_awaiting_preview_label")
            } actions: {
                Button("local_apps_open_preview") { path.append(.preview(appID)) }
                    .buttonStyle(.borderedProminent)
            }
        default:
            ContentUnavailableView {
                Label("local_apps_designer_not_ready", systemImage: "slider.horizontal.3")
            } description: {
                Text(steps.isEmpty ? String(localized: "local_apps_designer_waiting") : String(localized: "local_apps_designer_opening"))
            } actions: {
                Button("common_retry") { Task { await prepare() } }
            }
        }
    }

    private var values: [String: LocalAppDesignValue] { designer?.fields ?? [:] }

    private var canContinue: Bool {
        guard let currentStep else { return false }
        return LocalAppDesignerGate.isSatisfied(currentStep, values: values)
    }

    /// The step pills are ungated forward jumps, so the terminal action
    /// re-checks every step rather than trusting the currently visible one.
    /// `begin_planning` (state.rs) runs the identical `validate_answers`
    /// check server-side, but the client gate exists so a missing answer
    /// disables the button instead of costing a round trip.
    private var canConfirm: Bool {
        LocalAppDesignerGate.canConfirm(steps, values: values)
    }

    private var canAdvance: Bool {
        stepIndex + 1 < steps.count ? canContinue : canConfirm
    }

    private func prepare() async {
        guard store.designers[appID]?.interactionID == nil else { return }
        switch app?.workflow {
        case .awaitingSpecConfirmation:
            // `open_designer` is only legal from collecting_spec / generation_failed.
            // Once the gate is armed the app sits in awaiting_spec_confirmation and
            // the engine rejects it outright — which is what a relaunch used to
            // produce, because the client had no interaction_id yet and asked for a
            // gate that was already open. Refresh instead; the id arrives with the
            // re-announced gate (AppService::resync_pending_gates).
            await store.getDetails(appID: appID)
        case .collectingSpec:
            // Deliberately `getDetails`, NOT `openDesigner`. `open_designer`
            // (state.rs:491-501) transitions collecting_spec straight to
            // awaiting_spec_confirmation — arming the confirm gate and
            // moving the workflow off collecting_spec before the user has
            // even seen a question. `beginPlanning()` below requires
            // exactly collecting_spec, so that eager transition would make
            // every 生成方案 tap fail with workflow_state_invalid the
            // moment this screen was ever opened. Answering questions does
            // not need a gate at all: `update_draft` (state.rs) does not
            // check workflow state, so `getDetails` alone is enough to load
            // the current draft answers for editing.
            await store.getDetails(appID: appID)
        case .generationFailed:
            // RESTORED by Task 15. Task 14's review had folded this into the
            // no-op `default:` branch below, because back then
            // `store.confirmDesign`/`cancelDesign` had zero call sites
            // anywhere in `Sources/LocalApps/` — landing here in
            // `awaiting_spec_confirmation` (legal per the FSM, state.rs:
            // 491-501 explicitly admits `generation_failed`; its own test is
            // named `open_designer_reopens_a_failed_generation`) was a dead
            // end with no UI able to advance OR back out of it.
            // `LocalAppPlanConfirmView` is now that destination: `onConfirm`
            // re-confirms the SAME plan (`confirm_design` requires
            // `plan_for_revision == revision`, satisfied here since nothing
            // was edited since the original confirmation) and retries
            // generation; `onBack` calls
            // `cancel_design` back to `collecting_spec`, so a plan that
            // failed generation because the ANSWERS were wrong can actually
            // be fixed instead of retried verbatim forever (the only
            // recourse `LocalAppDetailView`'s "重试生成" — `retryGeneration`
            // → `retry_generation`, still present and unchanged — offers).
            await store.openDesigner(appID: appID)
        default:
            // `.authoringQuestionnaire`/`.planning` (an LLM round trip still
            // in flight) and `.questionnaireFailed`/`.planFailed` (a
            // failure awaiting a user-initiated retry, wired in
            // `unavailableView(for:)`) all deliberately no-op here: firing
            // a command automatically — especially a retry — without the
            // user asking would turn a workflow re-render into a retry
            // storm. `.task(id: app?.workflow)` already re-invokes
            // `prepare()` the moment the workflow actually changes, so a
            // busy state resolves itself without polling.
            return
        }
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
            Task { await beginPlanning() }
        }
    }

    /// The final step's action. Starts the plan-authoring LLM round trip
    /// (`collecting_spec -> planning`) — NOT `confirmDesign`/`confirm_design`,
    /// which is the LATER "confirm the derived plan" gate
    /// (`awaiting_spec_confirmation -> generating`, Task 15's screen) that
    /// `plan_ready` arms automatically once planning finishes. Stays on this
    /// screen either way: on success `.task(id: app?.workflow)` re-runs
    /// `prepare()` as soon as the workflow flips to `.planning`, and the
    /// busy `unavailableView(for:)` branch takes over without any
    /// navigation needed here.
    private func beginPlanning() async {
        confirming = true
        _ = await store.beginPlanning(appID: appID)
        confirming = false
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
            // The user explicitly chose to let the LLM decide — that IS a
            // complete answer, not a missing one (local-apps#questionnaire,
            // Task 1/13: the core gate treats `Deferred` as satisfying a
            // required field the same way).
            case .deferred:
                return true
            }
        }
    }
}

private struct DesignerStepHeader: View {
    @Environment(\.theme) private var theme
    let appName: String
    let steps: [LocalAppDesignStep]
    let selectedIndex: Int
    let onSelect: (Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(appName)
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
            // `.singleChoice`/`.multipleChoice` already render their
            // `allowsCustom`/`allowsDefer` affordances AS `DesignerFieldChips`
            // above (that's the field's whole editor for those two types).
            // Every other field type keeps its existing dedicated editor, so
            // this appends a second, options-less `DesignerFieldChips` row
            // underneath — `allowsDefer` is legal on ANY field
            // (`AppDesignField::allows_defer`, local-apps#questionnaire), not
            // only choice fields, so a shortText/color/etc. field can still
            // offer 「由你决定」 even though it has no options to chip.
            if showsSupplementaryChips {
                DesignerFieldChips(field: field, value: value, onChange: onChange)
            }
        }
    }

    private var showsSupplementaryChips: Bool {
        switch field.type {
        case .singleChoice, .multipleChoice: false
        default: field.allowsDefer || field.allowsCustom
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
        case .singleChoice, .multipleChoice:
            // The chips ARE the options here — `field.options` populates
            // `DesignerFieldChips.chipValues`, so this replaces the old
            // `Picker`/`LocalAppMultipleChoiceEditor` outright rather than
            // sitting alongside it (local-apps#questionnaire, Task 14).
            DesignerFieldChips(field: field, value: value, onChange: onChange)
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

/// Renders a field's options as tappable chips, plus (independently of the
/// options list) the `Other…` free-text box when `allowsCustom` and the
/// 「由你决定」 chip when `allowsDefer` — both mirror `AppDesignFieldDto`
/// (local-apps#questionnaire, Task 13/14). Used as the WHOLE editor for
/// `.singleChoice`/`.multipleChoice` fields (their `options` populate
/// `chipValues`) and, options-less, appended under every other field type
/// that sets `allowsCustom`/`allowsDefer` — both flags are legal on any
/// field, not only choice fields.
///
/// `chipValues`/`showsCustomInput`/`select(_:)` are plain, state-independent
/// computations over `field`/`value` so they are directly unit-testable
/// without a live view host (see `LocalAppsStoreTests.swift`).
struct DesignerFieldChips: View {
    enum Chip: Hashable {
        case option(String)
        case deferred
    }

    @Environment(\.theme) private var theme
    let field: LocalAppDesignField
    let value: LocalAppDesignValue?
    let onChange: (LocalAppDesignValue) -> Void

    @State private var customText: String

    init(
        field: LocalAppDesignField,
        value: LocalAppDesignValue? = nil,
        onChange: @escaping (LocalAppDesignValue) -> Void = { _ in }
    ) {
        self.field = field
        self.value = value
        self.onChange = onChange
        _customText = State(initialValue: DesignerFieldChips.initialCustomText(field: field, value: value))
    }

    /// The chip row: every declared option, then 「由你决定」 last when
    /// `allowsDefer`. `allowsCustom` does NOT add a chip — `showsCustomInput`
    /// below renders it as an always-visible text box instead, matching the
    /// engine's own framing ("Other…" is a box you type into, not a toggle).
    var chipValues: [Chip] {
        var chips = field.options.map { Chip.option($0.value) }
        if field.allowsDefer { chips.append(.deferred) }
        return chips
    }

    var showsCustomInput: Bool { field.allowsCustom }

    /// The option values known to this field — anything in a `.strings`/
    /// `.text` answer that is NOT among these is what `customText` holds.
    private var optionValues: Set<String> { Set(field.options.map(\.value)) }

    private var selectedOptions: [String] {
        switch value {
        case let .text(text): optionValues.contains(text) ? [text] : []
        case let .strings(values): values.filter(optionValues.contains)
        default: []
        }
    }

    private static func initialCustomText(field: LocalAppDesignField, value: LocalAppDesignValue?) -> String {
        let optionValues = Set(field.options.map(\.value))
        switch value {
        case let .text(text): return optionValues.contains(text) ? "" : text
        case let .strings(values): return values.first { !optionValues.contains($0) } ?? ""
        default: return ""
        }
    }

    func select(_ chip: Chip) {
        switch chip {
        case let .option(optionValue): onChange(toggled(optionValue))
        // Selecting 「由你决定」 sends `.deferred` — an ANSWER, not an
        // absence (local-apps#questionnaire, Task 1/13). It must never be
        // read back as "cleared the field".
        case .deferred: onChange(.deferred)
        }
    }

    private func toggled(_ optionValue: String) -> LocalAppDesignValue {
        guard field.type == .multipleChoice else { return .text(optionValue) }
        var values = selectedOptions
        if values.contains(optionValue) {
            values.removeAll { $0 == optionValue }
        } else {
            values.append(optionValue)
        }
        if !customText.isEmpty { values.append(customText) }
        return .strings(values)
    }

    private func commitCustom(_ text: String) {
        if field.type == .multipleChoice {
            var values = selectedOptions
            if !text.isEmpty { values.append(text) }
            onChange(.strings(values))
        } else {
            onChange(.text(text))
        }
    }

    private func isSelected(_ chip: Chip) -> Bool {
        switch chip {
        case let .option(optionValue): selectedOptions.contains(optionValue)
        case .deferred:
            if case .deferred = value { true } else { false }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !chipValues.isEmpty {
                ScrollView(.horizontal) {
                    HStack(spacing: 8) {
                        ForEach(chipValues, id: \.self) { chip in
                            Button(chipLabel(chip)) { select(chip) }
                                .buttonStyle(.bordered)
                                .tint(isSelected(chip) ? theme.accent : theme.text4)
                                .accessibilityIdentifier(chipAccessibilityID(chip))
                        }
                    }
                }
                .scrollIndicators(.hidden)
            }
            if showsCustomInput {
                TextField(
                    String(localized: "local_apps_custom_other"),
                    text: Binding(
                        get: { customText },
                        set: { newValue in
                            customText = newValue
                            commitCustom(newValue)
                        }
                    )
                )
                .textFieldStyle(.roundedBorder)
            }
        }
    }

    private func chipLabel(_ chip: Chip) -> String {
        switch chip {
        case let .option(optionValue):
            field.options.first { $0.value == optionValue }?.label ?? optionValue
        case .deferred:
            String(localized: "local_apps_value_deferred")
        }
    }

    private func chipAccessibilityID(_ chip: Chip) -> String {
        switch chip {
        case let .option(optionValue): "local-apps.chip.\(field.id).\(optionValue)"
        case .deferred: "local-apps.chip.\(field.id).deferred"
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
                    TextField("local_apps_field_name", text: $field.label)
                    Picker("local_apps_field_type", selection: $field.fieldType) {
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
                        label: "",
                        fieldType: .text,
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
                    // The final step starts an LLM round trip
                    // (`beginPlanning` → `begin_planning`), not a plain
                    // "next" — the copy must set that expectation instead of
                    // reusing "下一步" (local-apps#questionnaire, Task 14).
                    ? (isConfirming ? "local_apps_generating_plan" : "local_apps_generate_plan")
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

/// The "change description" escape hatch offered alongside `questionnaireFailed`'s
/// retry button. `update_brief` (state.rs) is legal from `questionnaire_failed`
/// (as well as `collecting_spec`/`plan_failed`) precisely so a bad brief that
/// produced a bad questionnaire is not a dead end — retrying with the SAME
/// brief would just reproduce the same failure if the brief itself was the
/// problem.
private struct DesignerBriefEditor: View {
    @Bindable var store: LocalAppsStore
    let appID: String

    @State private var brief: String
    @State private var submitting = false

    init(store: LocalAppsStore, appID: String, brief: String) {
        self.store = store
        self.appID = appID
        _brief = State(initialValue: brief)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("local_apps_brief", text: $brief, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .lineLimit(3 ... 6)
            Button(submitting ? "local_apps_updating_brief" : "local_apps_update_brief") {
                Task {
                    submitting = true
                    _ = await store.updateBrief(appID: appID, brief: brief)
                    submitting = false
                }
            }
            .buttonStyle(.bordered)
            .disabled(submitting || brief.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .frame(maxWidth: 320)
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
