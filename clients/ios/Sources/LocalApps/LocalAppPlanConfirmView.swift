import SwiftUI

/// The plan-confirmation sheet (local-apps#questionnaire, Task 15) — the
/// human gate between the LLM-derived plan (`awaiting_spec_confirmation`)
/// and code generation (`generating`). Read-only: nothing here mutates the
/// draft. Deliberately an independent sheet, not another questionnaire step
/// — it must not inherit `DesignerStepHeader`'s "上一步/下一步" editing
/// semantics, so it renders its own `NavigationStack` with exactly two
/// exits (`theSheetHasExactlyTwoExits`): `onBack` and `onConfirm`.
///
/// This view never touches `LocalAppsStore` directly — it takes plain
/// closures so it is constructable and testable (`LocalAppsFixtures.swift`)
/// without a live FFI-backed store. The caller (`LocalAppDesignerView`)
/// wires `onBack` to `store.cancelDesign(appID:)` (`cancel_design`:
/// `awaiting_spec_confirmation -> collecting_spec`, per `state.rs`) and
/// `onConfirm` to `store.confirmDesign(appID:)` (`confirm_design`, which
/// additionally enforces `plan_for_revision == revision` server-side —
/// mirrored client-side by only presenting this sheet while
/// `store.plans[appID] != nil`, since an answer edit voids the plan and the
/// resulting `appPlanChanged` event nils that entry out).
///
/// Never call `onConfirm` from anywhere but a user tap on the "确认并生成"
/// button. This is one of the two human confirmations the whole
/// conversational-design feature exists to preserve — nothing may
/// auto-advance it.
struct LocalAppPlanConfirmView: View {
    let plan: LocalAppPlan
    let onConfirm: () -> Void
    let onBack: () -> Void
    /// Caller-supplied guard against a double-tap firing two `confirm_design`
    /// commands while the first is still in flight. Defaulted so the
    /// brief's illustrative 3-arg construction (and the tests built on it)
    /// keeps compiling untouched.
    var isConfirming: Bool = false

    var body: some View {
        NavigationStack {
            List {
                Section("local_apps_plan_confirm_summary_header") {
                    Text(plan.summary)
                }
                Section("local_apps_plan_confirm_data_header") {
                    ForEach(plan.collections) { collection in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(collectionSummaryLine(collection))
                                .font(.subheadline.bold())
                            ForEach(collection.fields) { field in
                                Text(fieldDetailLine(field))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .padding(.vertical, 2)
                    }
                }
                // `id: \.self` would silently collapse a duplicated LLM-
                // authored capability/domain into one row — `validate_plan`
                // dedups collection/field ids but not these — dropping a row
                // the user is being asked to approve is the wrong failure on
                // a permissions disclosure, so index instead.
                Section("local_apps_plan_confirm_capabilities_header") {
                    ForEach(Array(capabilityLines.enumerated()), id: \.offset) { _, line in
                        Text(line)
                    }
                }
                // Silence about network access reads as an omission, not a
                // guarantee — this section always renders something, even
                // when `domains` is empty, rather than an empty list the
                // user might mistake for "not yet loaded."
                Section("local_apps_plan_confirm_domains_header") {
                    if plan.domains.isEmpty {
                        Label("local_apps_plan_confirm_no_domains", systemImage: "lock.shield")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(Array(plan.domains.enumerated()), id: \.offset) { _, domain in
                            Label(domain, systemImage: "network")
                        }
                    }
                }
            }
            .navigationTitle("local_apps_plan_confirm_title")
            .navigationBarTitleDisplayMode(.inline)
            .safeAreaInset(edge: .bottom) {
                HStack {
                    Button(actionTitles[0], action: onBack)
                        .buttonStyle(.bordered)
                        .disabled(isConfirming)
                        .accessibilityIdentifier("local-apps.plan-confirm.back")
                    Spacer()
                    Button(actionTitles[1], action: onConfirm)
                        .buttonStyle(.borderedProminent)
                        .disabled(isConfirming)
                        .accessibilityIdentifier("local-apps.plan-confirm.confirm")
                }
                .padding()
                .background(.bar)
            }
        }
    }

    /// "返回修改" then "确认并生成", in that exact order — the sheet's whole
    /// exit contract (`theSheetHasExactlyTwoExits`). Plain `String`s (not
    /// `LocalizedStringKey` literals) so the `Button` inits below render them
    /// verbatim instead of re-resolving them as a second, redundant lookup.
    var actionTitles: [String] {
        [
            String(localized: "local_apps_plan_confirm_back"),
            String(localized: "local_apps_plan_confirm_confirm"),
        ]
    }

    /// Flattened text projection of every section below the prose summary,
    /// built from the exact same per-row formatters the `body` above
    /// renders — so a test against this array is a test against what the
    /// user actually sees, not a parallel description of it.
    var summaryLines: [String] {
        var lines = [plan.summary]
        for collection in plan.collections {
            lines.append(collectionSummaryLine(collection))
            lines.append(contentsOf: collection.fields.map(fieldDetailLine))
        }
        lines.append(contentsOf: capabilityLines)
        lines.append(contentsOf: domainLines)
        return lines
    }

    /// One line per collection naming it AND every field it holds — the
    /// data-table's group header. Carries both the collection's and its
    /// fields' identifiers (not just their display labels) because this is
    /// what will literally exist in the generated app's manifest.
    private func collectionSummaryLine(_ collection: LocalAppDataCollection) -> String {
        let fieldIDs = collection.fields.map(\.id).joined(separator: ", ")
        return "\(collection.label) (\(collection.id)): \(fieldIDs)"
    }

    private func fieldDetailLine(_ field: LocalAppDataField) -> String {
        let base = "\(field.label) (\(field.id)) — \(field.fieldType.rawValue)"
        return field.required ? base + " · " + String(localized: "local_apps_field_required") : base
    }

    private func capabilityLine(_ capability: LocalAppCapabilityKind) -> String {
        switch capability {
        case .dataMutation: String(localized: "local_apps_plan_confirm_capability_data_mutation")
        case .uiControl: String(localized: "local_apps_plan_confirm_capability_ui_control")
        case .networkDomain: String(localized: "local_apps_plan_confirm_capability_network_domain")
        case .restoreCheckpoint: String(localized: "local_apps_plan_confirm_capability_restore_checkpoint")
        }
    }

    /// Mirrors `domainLines` below: says so explicitly
    /// (`local_apps_plan_confirm_no_capabilities`) when the plan requests no
    /// capabilities at all, rather than rendering an empty section a reader
    /// could mistake for "not yet loaded."
    private var capabilityLines: [String] {
        plan.capabilities.isEmpty
            ? [String(localized: "local_apps_plan_confirm_no_capabilities")]
            : plan.capabilities.map(capabilityLine)
    }

    private var domainLines: [String] {
        plan.domains.isEmpty ? [String(localized: "local_apps_plan_confirm_no_domains")] : plan.domains
    }
}
