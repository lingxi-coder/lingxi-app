import SwiftUI

private func approvalDigestSummary(_ value: String) -> String {
  guard value.count > 12 else { return value }
  return String(value.prefix(12))
}

private func approvalReceiptExpiryText(_ key: String, _ date: Date) -> String {
  String(
    format: NSLocalizedString(key, comment: ""),
    date.formatted(date: .abbreviated, time: .shortened))
}

private func approvalSurfaceTitle(_ surface: LocalAppRuntimeProfileSurface) -> String {
  switch surface {
  case .dom:
    String(localized: "local_apps_runtime_profile_surface_dom")
  case .canvas:
    String(localized: "local_apps_runtime_profile_surface_canvas")
  }
}

struct LocalAppCreateConfirmationSheet: View {
  @Bindable var store: LocalAppsStore
  let prompt: LocalAppCreateConfirmationPrompt

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          SettingsSection(label: String(localized: "local_apps_create_confirm_title")) {
            SettingsRow(
              label: String(localized: "local_apps_create_confirm_app_name"), value: prompt.name,
              chevron: false)
            SettingsRow(
              label: String(localized: "local_apps_create_confirm_app_brief"), sub: prompt.brief,
              chevron: false)
            SettingsRow(
              label: String(localized: "local_apps_create_confirm_selected_template"),
              sub: prompt.selectedTemplate.summary, value: prompt.selectedTemplate.templateID,
              chevron: false)
            SettingsRow(
              label: String(localized: "local_apps_create_confirm_runtime_profile"),
              sub: runtimeProfileSummary,
              value: prompt.runtimeProfile.family.title,
              chevron: false,
              isLast: true
            )
          }

          SettingsSection(label: String(localized: "local_apps_create_confirm_agent_reason")) {
            Text(prompt.reason)
              .font(.body)
              .frame(maxWidth: .infinity, alignment: .leading)
              .padding(.horizontal, 14)
              .padding(.vertical, 11)
          }

          if !prompt.rejected.isEmpty {
            SettingsSection(
              label: String(localized: "local_apps_create_confirm_rejected_candidates")
            ) {
              ForEach(Array(prompt.rejected.enumerated()), id: \.element.id) { index, rejected in
                SettingsRow(
                  label: rejected.templateID,
                  sub: rejected.reason,
                  value: approvalSurfaceTitle(prompt.selectedTemplate.surface),
                  chevron: false,
                  isLast: index == prompt.rejected.count - 1
                )
              }
            }
          }

          SettingsSection(label: String(localized: "local_apps_create_confirm_initial_tools")) {
            if prompt.initialTools.isEmpty {
              SettingsRow(label: String(localized: "common_none"), chevron: false, isLast: true)
            } else {
              ForEach(Array(prompt.initialTools.enumerated()), id: \.element.id) { index, tool in
                SettingsRow(
                  label: tool.name,
                  sub: tool.title ?? tool.description ?? String(localized: "common_none"),
                  value: tool.ceilingSummary,
                  chevron: false,
                  isLast: index == prompt.initialTools.count - 1
                )
              }
            }
          }

          SettingsSection(label: String(localized: "local_apps_create_confirm_required_gates")) {
            if prompt.requiredGates.isEmpty {
              SettingsRow(label: String(localized: "common_none"), chevron: false, isLast: true)
            } else {
              ForEach(Array(prompt.requiredGates.enumerated()), id: \.element.id) { index, gate in
                SettingsRow(
                  label: gate.label,
                  labelView: AnyView(
                    VStack(alignment: .leading, spacing: 6) {
                      Text(gate.label).font(.system(size: 14, weight: .medium))
                      HStack(spacing: 8) {
                        LocalAppPublicationBadgeView(badge: gate.badge)
                        if !gate.available {
                          Text("runner unavailable")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        }
                      }
                    }
                  ),
                  sub: gate.detail ?? gate.status.badge.label,
                  chevron: false,
                  isLast: index == prompt.requiredGates.count - 1
                )
              }
            }
          }

          if let receipt = prompt.receipt {
            SettingsSection(label: "Receipt") {
              SettingsRow(
                label: "Approval", value: approvalDigestSummary(receipt.approvalContractSHA256),
                chevron: false)
              SettingsRow(
                label: "Candidate", value: approvalDigestSummary(receipt.candidateDigest),
                chevron: false)
              SettingsRow(
                label: approvalReceiptExpiryText(
                  "local_apps_create_confirm_receipt_expires_fmt", receipt.expiresAt),
                value: receipt.superseded
                  ? String(localized: "local_apps_mcp_proposal_receipt_superseded") : nil,
                chevron: false,
                isLast: true
              )
            }
          }
        }
        .padding()
      }
      .navigationTitle("local_apps_create_confirm_title")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("local_apps_create_confirm_reject") { resolve(false) }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button("local_apps_create_confirm_approve") { resolve(true) }
        }
      }
    }
    .interactiveDismissDisabled()
    .presentationDetents([.medium, .large])
    .presentationDragIndicator(.visible)
    .accessibilityIdentifier("local-apps.create-confirm.\(prompt.id)")
  }

  private var runtimeProfileSummary: String {
    [
      "r\(prompt.runtimeProfile.revision)",
      approvalSurfaceTitle(prompt.runtimeProfile.surface),
      approvalDigestSummary(prompt.runtimeProfile.contractSHA256),
    ].joined(separator: " · ")
  }

  private func resolve(_ approved: Bool) {
    Task { await store.resolvePendingCreateConfirmation(approved) }
  }
}

struct LocalAppMcpProposalApprovalSheet: View {
  @Bindable var store: LocalAppsStore
  let prompt: LocalAppMcpProposalApprovalPrompt

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          SettingsSection(label: String(localized: "local_apps_mcp_proposal_title")) {
            SettingsRow(label: "App", value: prompt.appID, chevron: false)
            SettingsRow(
              label: "Proposal", value: approvalDigestSummary(prompt.proposalSHA256), chevron: false
            )
            SettingsRow(
              label: "Surface", value: approvalDigestSummary(prompt.toolSurfaceSHA256),
              chevron: false)
            SettingsRow(
              label: "Approval", value: approvalDigestSummary(prompt.approvalContractSHA256),
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
            SettingsSection(label: String(localized: "local_apps_create_confirm_required_gates")) {
              ForEach(Array(prompt.pendingGates.enumerated()), id: \.element.id) { index, gate in
                SettingsRow(
                  label: gate.label,
                  labelView: AnyView(
                    VStack(alignment: .leading, spacing: 6) {
                      Text(gate.label).font(.system(size: 14, weight: .medium))
                      HStack(spacing: 8) {
                        LocalAppPublicationBadgeView(badge: gate.badge)
                        if !gate.available {
                          Text("runner unavailable")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                        }
                      }
                    }
                  ),
                  sub: gate.detail ?? gate.status.badge.label,
                  chevron: false,
                  isLast: index == prompt.pendingGates.count - 1
                )
              }
            }
          }

          if let receipt = prompt.receipt {
            SettingsSection(label: "Receipt") {
              SettingsRow(label: "Workflow", value: receipt.workflowRunID, chevron: false)
              SettingsRow(
                label: "Candidate", value: approvalDigestSummary(receipt.candidateDigest),
                chevron: false)
              SettingsRow(
                label: approvalReceiptExpiryText(
                  "local_apps_mcp_proposal_receipt_expires_fmt", receipt.expiresAt),
                sub: receipt.superseded
                  ? String(localized: "local_apps_mcp_proposal_receipt_superseded") : nil,
                chevron: false,
                isLast: true
              )
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
      toolValue(label: "Title", value: surface.title ?? String(localized: "common_none"))
      toolValue(
        label: "Description", value: surface.description ?? String(localized: "common_none"))
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
