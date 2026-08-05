// PermissionPrompt.swift — SHIP-BLOCKER #3.
//
// The SwiftUI allow/deny surface for engine-parked permission requests, the iOS
// mirror of the Electron renderer's `PermissionPrompt.tsx` (7.3).
//
// The engine's adapter permission gate emits a `PermissionRequest` whenever a
// tool needs approval (e.g. a Write/Bash invocation) and parks the turn on a
// oneshot until the user answers. `EnginePermissionSink` forwards each request
// onto `ConversationModel.pendingPermissions`; this view renders the head as a
// modal with three actions — Deny / Allow always / Allow once — each resolving
// the park by submitting `ClientCommand.approvePermission` / `denyPermission`
// (correlated by `requestId`) back through the `MobileEngineHandle`.
//
// Compiled only when the engine bindings are linked: the request DTOs are
// UniFFI types, and the mock source never parks a turn on a permission gate.

import SwiftUI

#if canImport(engine_mobileFFI)

    /// A modal prompt for one engine-parked permission request (the head of the
    /// queue). Renders nothing when there is no pending request.
    struct PermissionPrompt: View {
        @Environment(\.theme) private var t

        /// The head request to render, or `nil` to render nothing.
        let pending: PendingPermission?
        /// Approve the request (allow-once / allow-always).
        let onApprove: (UInt64, PermissionResponseDto) -> Void
        /// Deny the request.
        let onDeny: (UInt64) -> Void

        var body: some View {
            if let pending {
                let copy = Self.describe(pending.kind)
                ZStack {
                    // Scrim: dims the chat behind the modal. Tapping it does NOT
                    // dismiss — a permission request must be answered explicitly
                    // (an accidental tap-away can't silently deny a tool).
                    Color.black.opacity(0.32)
                        .ignoresSafeArea()

                    VStack(spacing: 0) {
                        VStack(alignment: .leading, spacing: 8) {
                            if let worker = pending.worker {
                                workerChip(worker)
                            }
                            Text(copy.title)
                                .font(.system(size: 16, weight: .semibold))
                                .foregroundColor(t.text)
                                .fixedSize(horizontal: false, vertical: true)
                            if !copy.detail.isEmpty {
                                detailBlock(copy.detail)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 20)
                        .padding(.top, 20)
                        .padding(.bottom, 16)

                        Divider().overlay(t.border)

                        actionRow(requestId: pending.requestId)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)
                            .background(t.surface)
                    }
                    .frame(maxWidth: 420)
                    .background(t.windowBg)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .overlay(RoundedRectangle(cornerRadius: 16).stroke(t.border, lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.34), radius: 24, x: 0, y: 18)
                    .padding(.horizontal, 24)
                }
                .transition(.opacity)
            }
        }

        // MARK: pieces

        /// The sub-agent attribution chip (always absent in the foundation, but
        /// rendered for parity with the Electron prompt should a worker arrive).
        private func workerChip(_ worker: WorkerInfoDto) -> some View {
            HStack(spacing: 6) {
                Circle()
                    .fill(t.accent)
                    .frame(width: 7, height: 7)
                Text(worker.team.map { "\(worker.name) · \($0)" } ?? worker.name)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(t.text3)
            }
        }

        /// The monospaced, scrollable detail block (the tool-input preview / plan).
        private func detailBlock(_ detail: String) -> some View {
            ScrollView(showsIndicators: true) {
                Text(detail)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundColor(t.text2)
                    .lineSpacing(3)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
            }
            .frame(maxHeight: 180)
            .background(t.surface)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(t.border, lineWidth: 0.5))
        }

        /// Deny / Allow always / Allow once — mirroring the Electron button order
        /// and response mapping. Allow-once is the accent (primary) action.
        private func actionRow(requestId: UInt64) -> some View {
            HStack(spacing: 8) {
                promptButton(String(localized: "permission_deny"), tint: t.danger, filled: false) {
                    onDeny(requestId)
                }
                promptButton(String(localized: "permission_allow_always"), tint: t.text2, filled: false) {
                    onApprove(requestId, .allowAlways)
                }
                promptButton(String(localized: "permission_allow"), tint: .white, filled: true) {
                    onApprove(requestId, .allowOnce)
                }
            }
        }

        private func promptButton(
            _ label: String, tint: Color, filled: Bool, action: @escaping () -> Void
        ) -> some View {
            Button(action: action) {
                Text(label)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(tint)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 9)
                    .background(filled ? t.accent : Color.clear)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(filled ? t.accent : t.border, lineWidth: 0.5)
                    )
            }
            .buttonStyle(.plain)
        }

        // MARK: copy

        /// Human title + detail for each request kind, matching the Electron
        /// `describe` switch. `@unknown default` keeps a future kind renderable.
        static func describe(_ kind: PermissionKindDto) -> (title: String, detail: String) {
            switch kind {
            case let .toolUseConfirm(toolName, toolInputJson, _):
                return (String(localized: "permission_allow_tool \(toolName)"), previewToolInput(toolInputJson))
            case let .exitPlanMode(plan):
                return (String(localized: "permission_exit_plan_mode"), plan)
            case .bypassPermissionsMode:
                return (String(localized: "permission_bypass_confirmation_title"), String(localized: "permission_bypass_confirmation_detail"))
            @unknown default:
                return (String(localized: "permission_request_title"), "")
            }
        }

        /// Best-effort, never-throwing one-line preview of a tool's JSON input —
        /// surfaces the most telling field (command / path / pattern / query),
        /// falling back to the raw JSON. Mirrors the Electron `previewToolInput`.
        static func previewToolInput(_ inputJson: String) -> String {
            guard !inputJson.isEmpty,
                let data = inputJson.data(using: .utf8),
                let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            else { return inputJson }
            for key in ["command", "file_path", "path", "pattern", "query"] {
                if let value = obj[key] as? String, !value.isEmpty {
                    return value
                }
            }
            return inputJson
        }
    }

#endif
