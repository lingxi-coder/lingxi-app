import SwiftUI

private func approvalDigestSummary(_ value: String) -> String {
  guard value.count > 12 else { return value }
  return String(value.prefix(12))
}

/// The host (`pending_verification_gates` in `local_apps_host.rs`) always
/// sends `label`/`detail` as fixed English strings — it has no notion of the
/// client's locale. Map the two `gate_id`s it currently defines to the
/// client's own catalog; an unrecognized future `gate_id` falls back to the
/// engine's raw string rather than showing nothing.
private func localizedGateLabel(_ gate: LocalAppGateStatus) -> String {
  switch gate.gateID {
  case "mcp_qa":
    String(localized: "local_apps_approval_gate_mcp_qa_label")
  case "ui_runner":
    String(localized: "local_apps_approval_gate_ui_runner_label")
  default:
    gate.label
  }
}

private func localizedGateDetail(_ gate: LocalAppGateStatus) -> String? {
  if gate.gateID == "ui_runner", !gate.available {
    return String(localized: "local_apps_approval_gate_ui_runner_unavailable_detail")
  }
  return gate.detail
}

struct LocalAppMcpProposalApprovalSheet: View {
  @Bindable var store: LocalAppsStore
  let prompt: LocalAppMcpProposalApprovalPrompt

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          SettingsSection(label: String(localized: "local_apps_mcp_proposal_title")) {
            SettingsRow(
              label: String(localized: "local_apps_mcp_proposal_field_app"), value: prompt.appID,
              chevron: false)
            SettingsRow(
              label: String(localized: "local_apps_mcp_proposal_field_proposal"),
              value: approvalDigestSummary(prompt.proposalSHA256), chevron: false
            )
            SettingsRow(
              label: String(localized: "local_apps_mcp_proposal_field_surface"),
              value: approvalDigestSummary(prompt.toolSurfaceSHA256),
              chevron: false)
            SettingsRow(
              label: String(localized: "local_apps_mcp_proposal_field_approval_contract"),
              value: approvalDigestSummary(prompt.approvalContractSHA256),
              chevron: false, isLast: true)
          }

          SettingsSection {
            Text(prompt.summary)
              .font(.body)
              .frame(maxWidth: .infinity, alignment: .leading)
              .padding(.horizontal, 14)
              .padding(.vertical, 11)
          }

          ForEach(prompt.toolDiffs) { diff in
            SettingsSection(label: "\(diff.kind.label) · \(diff.name)") {
              if let before = diff.before {
                toolSurfaceBlock(
                  title: String(localized: "local_apps_mcp_proposal_before"), surface: before)
              }
              if let after = diff.after {
                toolSurfaceBlock(
                  title: String(localized: "local_apps_mcp_proposal_after"), surface: after)
              }
              if !diff.changedFields.isEmpty {
                SettingsRow(
                  label: diff.changedFields.map(\.label).joined(separator: ", "),
                  chevron: false,
                  isLast: true
                )
              }
            }
          }

          if !prompt.requiredFlowChanges.isEmpty {
            SettingsSection(
              label: String(localized: "local_apps_mcp_proposal_required_flow_changes")
            ) {
              ForEach(Array(prompt.requiredFlowChanges.enumerated()), id: \.offset) { index, item in
                SettingsRow(
                  label: item, chevron: false, isLast: index == prompt.requiredFlowChanges.count - 1
                )
              }
            }
          }

          if !prompt.excludedCapabilities.isEmpty {
            SettingsSection(
              label: String(localized: "local_apps_mcp_proposal_excluded_capabilities")
            ) {
              ForEach(Array(prompt.excludedCapabilities.enumerated()), id: \.offset) {
                index, item in
                SettingsRow(
                  label: item, chevron: false,
                  isLast: index == prompt.excludedCapabilities.count - 1)
              }
            }
          }

          if !prompt.pendingGates.isEmpty {
            SettingsSection(label: String(localized: "local_apps_approval_required_gates")) {
              ForEach(Array(prompt.pendingGates.enumerated()), id: \.element.id) { index, gate in
                SettingsRow(
                  label: localizedGateLabel(gate),
                  labelView: AnyView(
                    VStack(alignment: .leading, spacing: 6) {
                      Text(localizedGateLabel(gate)).font(.system(size: 14, weight: .medium))
                      HStack(spacing: 8) {
                        LocalAppPublicationBadgeView(badge: gate.badge)
                        if !gate.available {
                          Text("local_apps_approval_gate_runner_unavailable")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        }
                      }
                    }
                  ),
                  sub: localizedGateDetail(gate) ?? gate.status.badge.label,
                  chevron: false,
                  isLast: index == prompt.pendingGates.count - 1
                )
              }
            }
          }
        }
        .padding()
      }
      .navigationTitle("local_apps_mcp_proposal_title")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("local_apps_mcp_proposal_reject") { resolve(false) }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("local_apps_mcp_proposal_approve") { resolve(true) }
        }
      }
    }
    .interactiveDismissDisabled()
    .presentationDetents([.medium, .large])
    .presentationDragIndicator(.visible)
    .accessibilityIdentifier("local-apps.mcp-proposal.\(prompt.id)")
  }

  @ViewBuilder
  private func toolSurfaceBlock(title: String, surface: LocalAppMcpToolSurface) -> some View {
    VStack(alignment: .leading, spacing: 10) {
      Text(title)
        .font(.footnote.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
        .padding(.top, 12)
      toolValue(label: String(localized: "settings_display_name"), value: surface.name)
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_title"),
        value: surface.title ?? String(localized: "common_none"))
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_description"),
        value: surface.description ?? String(localized: "common_none"))
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_input_schema"),
        value: surface.inputSchemaSummary)
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_output_schema"),
        value: surface.outputSchemaSummary ?? String(localized: "common_none"))
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_annotations"),
        value: surface.annotationsSummary ?? String(localized: "common_none"))
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_execution"),
        value: surface.executionSummary ?? String(localized: "common_none"))
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_visible_meta"),
        value: surface.visibleMetaSummary ?? String(localized: "common_none"))
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_semantic_flow"),
        value: surface.semanticFlowSummary)
      toolValue(
        label: String(localized: "local_apps_mcp_proposal_field_permission_ceiling"),
        value: surface.ceilingSummary)
    }
  }

  @ViewBuilder
  private func toolValue(label: String, value: String) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(label)
        .font(.caption.weight(.medium))
        .foregroundStyle(.secondary)
      Text(value)
        .font(.system(size: 11.5, design: .monospaced))
        .textSelection(.enabled)
    }
    .padding(.horizontal, 14)
    .padding(.bottom, 10)
  }

  private func resolve(_ approved: Bool) {
    Task { await store.resolvePendingMcpProposalApproval(approved) }
  }
}
