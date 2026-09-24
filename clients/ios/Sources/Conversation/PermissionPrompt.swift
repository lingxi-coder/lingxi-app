// PermissionPrompt.swift — SHIP-BLOCKER #3.
//
// The SwiftUI sheet for engine-parked permission requests, the iOS
// mirror of the Electron renderer's `PermissionPrompt.tsx` (7.3).
//
// The engine's adapter permission gate emits a `PermissionRequest` whenever a
// tool needs approval (e.g. a Write/Bash invocation) and parks the turn on a
// oneshot until the user answers. `EnginePermissionSink` forwards each request
// onto `ConversationModel.pendingPermissions`; the app-level host presents the
// head as a native sheet above the current chat surface with actions — Deny /
// Allow always / Allow once — each resolving
// the park by submitting `ClientCommand.approvePermission` / `denyPermission`
// (correlated by `requestId`) back through the `MobileEngineHandle`.
//
// Compiled only when the engine bindings are linked: the request DTOs are
// UniFFI types, and the mock source never parks a turn on a permission gate.

import SwiftUI
import UIKit

#if canImport(engine_mobileFFI)

    extension Notification.Name {
        /// Posted by SwiftUI-owned sheets so the root permission-sheet host can
        /// retry after UIKit attaches the active presenter.
        static let lingxiPermissionPresentationContextChanged = Notification.Name(
            "LingxiPermissionPresentationContextChanged"
        )
    }

    enum PermissionPromptPresentationPolicy {
        static func canPresent(
            sceneActivationState: UIScene.ActivationState,
            presenterIsAttached: Bool
        ) -> Bool {
            sceneActivationState == .foregroundActive && presenterIsAttached
        }
    }

    /// Observes the engine-scoped queue and presents its head as a sheet.
    /// The root-mounted host presents it over whichever native surface is active.
    struct EnginePermissionPromptHost: View {
        @ObservedObject var model: ConversationModel
        @Environment(\.theme) private var theme
        @Environment(\.locale) private var locale

        let onApprove: (UInt64, PermissionResponseDto) -> Void
        let onDeny: (UInt64) -> Void

        init(
            model: ConversationModel,
            onApprove: @escaping (UInt64, PermissionResponseDto) -> Void,
            onDeny: @escaping (UInt64) -> Void
        ) {
            self.model = model
            self.onApprove = onApprove
            self.onDeny = onDeny
        }

        var body: some View {
            PermissionPromptPresentationBridge(
                pending: model.pendingPermissions.first,
                theme: theme,
                locale: locale,
                onApprove: onApprove,
                onDeny: onDeny
            )
            .frame(width: 0, height: 0)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
    }

    /// UIKit presentation is intentionally limited to presentation mechanics;
    /// permission delivery and resolution continue to use the existing engine
    /// callback and command paths.
    private struct PermissionPromptPresentationBridge: UIViewControllerRepresentable {
        let pending: PendingPermission?
        let theme: Palette
        let locale: Locale
        let onApprove: (UInt64, PermissionResponseDto) -> Void
        let onDeny: (UInt64) -> Void

        func makeCoordinator() -> Coordinator {
            Coordinator()
        }

        func makeUIViewController(context _: Context) -> UIViewController {
            let controller = UIViewController()
            controller.view.isHidden = true
            return controller
        }

        func updateUIViewController(_ controller: UIViewController, context: Context) {
            context.coordinator.update(
                pending: pending,
                theme: theme,
                locale: locale,
                fallbackPresenter: controller,
                onApprove: onApprove,
                onDeny: onDeny
            )
        }

        static func dismantleUIViewController(
            _: UIViewController,
            coordinator: Coordinator
        ) {
            coordinator.dismiss(animated: false)
        }

        @MainActor
        final class Coordinator: NSObject {
            private var hostingController: UIHostingController<AnyView>?
            private var deferredContent: AnyView?
            private weak var fallbackPresenter: UIViewController?
            private var presentationInFlight = false
            private var presentationGeneration = 0
            private var retryScheduled = false
            private var consumedRunLoopRetryGeneration: Int?

            override init() {
                super.init()
                NotificationCenter.default.addObserver(
                    self,
                    selector: #selector(sceneDidActivate),
                    name: UIScene.didActivateNotification,
                    object: nil
                )
                NotificationCenter.default.addObserver(
                    self,
                    selector: #selector(windowPresentationContextChanged),
                    name: UIWindow.didBecomeVisibleNotification,
                    object: nil
                )
                NotificationCenter.default.addObserver(
                    self,
                    selector: #selector(windowPresentationContextChanged),
                    name: UIWindow.didBecomeKeyNotification,
                    object: nil
                )
                NotificationCenter.default.addObserver(
                    self,
                    selector: #selector(presentationContextChanged),
                    name: .lingxiPermissionPresentationContextChanged,
                    object: nil
                )
            }

            deinit {
                NotificationCenter.default.removeObserver(self)
            }

            func update(
                pending: PendingPermission?,
                theme: Palette,
                locale: Locale,
                fallbackPresenter: UIViewController,
                onApprove: @escaping (UInt64, PermissionResponseDto) -> Void,
                onDeny: @escaping (UInt64) -> Void
            ) {
                guard let pending else {
                    dismiss(animated: true)
                    return
                }

                let content = AnyView(
                    PermissionPrompt(
                        pending: pending,
                        onApprove: onApprove,
                        onDeny: onDeny
                    )
                    .environment(\.theme, theme)
                    .environment(\.locale, locale)
                )

                deferredContent = content
                self.fallbackPresenter = fallbackPresenter

                if let hostingController {
                    hostingController.rootView = content
                    if presentationInFlight || hostingController.presentingViewController != nil {
                        return
                    }
                    self.hostingController = nil
                }

                attemptPresentation()
            }

            func dismiss(animated: Bool) {
                presentationGeneration &+= 1
                deferredContent = nil
                fallbackPresenter = nil
                presentationInFlight = false
                consumedRunLoopRetryGeneration = nil
                guard let hostingController else { return }
                self.hostingController = nil
                hostingController.dismiss(animated: animated)
            }

            @objc private func sceneDidActivate() {
                attemptPresentation()
            }

            @objc private func windowPresentationContextChanged() {
                attemptPresentation()
            }

            @objc private func presentationContextChanged() {
                // SwiftUI posts the composer/summary state change before UIKit
                // attaches the resulting sheet. Defer one main-queue turn so
                // `Presenter.topViewController()` observes the settled cover.
                DispatchQueue.main.async { [weak self] in
                    self?.attemptPresentation()
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
                    self?.attemptPresentation()
                }
            }

            private func attemptPresentation() {
                guard hostingController == nil,
                      !presentationInFlight,
                      let deferredContent
                else { return }
                guard let presenter = activeAttachedPresenter() else {
                    scheduleRetry()
                    return
                }
                let host = UIHostingController(rootView: deferredContent)
                host.view.backgroundColor = .clear
                host.modalPresentationStyle = .pageSheet
                host.isModalInPresentation = true
                host.sheetPresentationController?.detents = [.large()]
                host.sheetPresentationController?.prefersGrabberVisible = true
                host.sheetPresentationController?.preferredCornerRadius = 28

                presentationGeneration &+= 1
                let generation = presentationGeneration
                hostingController = host
                presentationInFlight = true
                present(host, from: presenter, generation: generation)
            }

            private func topViewController(in scene: UIWindowScene) -> UIViewController? {
                let window = scene.windows.first(where: \.isKeyWindow) ?? scene.windows.first
                var top = window?.rootViewController
                while let presented = top?.presentedViewController {
                    top = presented
                }
                return top
            }

            private func scheduleRetry() {
                guard !retryScheduled, deferredContent != nil else { return }
                guard consumedRunLoopRetryGeneration != presentationGeneration else { return }
                consumedRunLoopRetryGeneration = presentationGeneration
                retryScheduled = true
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.retryScheduled = false
                    self.attemptPresentation()
                }
            }

            private func activeAttachedPresenter() -> UIViewController? {
                let fallback = fallbackPresenter
                let fallbackScene = fallback?.viewIfLoaded?.window?.windowScene
                let presenter: UIViewController?
                let scene: UIWindowScene?
                if let fallbackScene,
                   fallbackScene.activationState == .foregroundActive,
                   let top = topViewController(in: fallbackScene)
                {
                    presenter = top
                    scene = fallbackScene
                } else if let fallback,
                   let fallbackScene,
                   fallbackScene.activationState == .foregroundActive {
                    var top = fallback
                    while let presented = top.presentedViewController {
                        top = presented
                    }
                    presenter = top
                    scene = fallbackScene
                } else if let top = Presenter.topViewController(),
                          let topScene = top.viewIfLoaded?.window?.windowScene,
                          topScene.activationState == .foregroundActive {
                    presenter = top
                    scene = topScene
                } else {
                    return nil
                }

                guard let presenter,
                      let scene,
                      PermissionPromptPresentationPolicy.canPresent(
                          sceneActivationState: scene.activationState,
                          presenterIsAttached: presenter.viewIfLoaded?.window != nil
                      )
                else { return nil }
                return presenter
            }

            private func present(
                _ host: UIHostingController<AnyView>,
                from presenter: UIViewController,
                generation: Int
            ) {
                // If another screen is mid-transition, continue from UIKit's
                // completion callback instead of polling presentation state.
                // UIKit can expose a stale transition coordinator even when
                // animations are globally disabled (notably in tests and
                // accessibility-driven presentation). Its completion is not
                // guaranteed to fire, so present synchronously in that mode.
                if UIView.areAnimationsEnabled,
                   let transition = presenter.transitionCoordinator
                {
                    transition.animate(alongsideTransition: nil) { [weak self] _ in
                        guard let self,
                              self.presentationGeneration == generation,
                              self.hostingController === host
                        else { return }
                        guard let settledPresenter = self.activeAttachedPresenter() else {
                            self.presentationInFlight = false
                            self.hostingController = nil
                            self.scheduleRetry()
                            return
                        }
                        self.performPresentation(
                            host,
                            from: settledPresenter,
                            generation: generation
                        )
                    }
                    return
                }

                guard presentationGeneration == generation,
                      hostingController === host
                else { return }
                performPresentation(host, from: presenter, generation: generation)
            }

            private func performPresentation(
                _ host: UIHostingController<AnyView>,
                from presenter: UIViewController,
                generation: Int
            ) {
                presenter.present(host, animated: true) { [weak self, weak host] in
                    guard let self,
                          let host,
                          self.presentationGeneration == generation,
                          self.hostingController === host
                    else { return }
                    self.presentationInFlight = false
                    if host.presentingViewController == nil {
                        self.hostingController = nil
                        self.scheduleRetry()
                    }
                }
            }
        }
    }

    /// The sheet content for one engine-parked permission request (the head of
    /// the queue). Renders nothing when there is no pending request.
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
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Image(systemName: Self.isElevatedRisk(pending) ? "shield.lefthalf.filled" : "hand.raised.fill")
                            .font(.system(size: 17, weight: .medium))
                            .foregroundStyle(Self.isElevatedRisk(pending) ? t.danger : t.text2)
                            .frame(width: 34, height: 34)
                            .background(t.surfaceActive, in: Circle())
                            .accessibilityHidden(true)
                        Text(String(localized: "permission_request_title"))
                            .font(.headline)
                            .foregroundStyle(t.text)
                        Spacer(minLength: 0)
                        Button {
                            onDeny(pending.requestId)
                        } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 14, weight: .medium))
                                .foregroundStyle(t.text3)
                                .frame(width: 36, height: 36)
                                .contentShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(String(localized: "permission_deny"))
                        .accessibilityIdentifier("permission.deny.close")
                    }
                    .padding(.horizontal, 20)
                    .padding(.top, 18)
                    .padding(.bottom, 12)

                    ScrollView {
                        VStack(alignment: .leading, spacing: 14) {
                            Text(copy.title)
                                .font(.title2.weight(.semibold))
                                .foregroundStyle(t.text)
                                .fixedSize(horizontal: false, vertical: true)
                            Text(copy.summary)
                                .font(.subheadline)
                                .foregroundStyle(t.text2)
                                .fixedSize(horizontal: false, vertical: true)
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Text(copy.detailLabel)
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(t.text)
                                Text(copy.detailCaption)
                                    .font(.caption)
                                    .foregroundStyle(t.text3)
                            }
                            if let worker = pending.worker {
                                workerChip(worker)
                            }
                            if !copy.detail.isEmpty {
                                if case .exitPlanMode = pending.kind {
                                    PlanDocumentCard(document: PlanDocument(markdown: copy.detail, isWriting: false))
                                } else {
                                    detailBlock(copy.detail)
                                }
                            }
                            HStack(alignment: .top, spacing: 8) {
                                Image(systemName: "info.circle")
                                Text(Self.riskCopy(for: pending))
                            }
                            .font(.caption)
                            .foregroundStyle(t.text3)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 20)
                        .padding(.bottom, 18)
                    }
                    .scrollIndicators(.visible)
                    .scrollBounceBehavior(.basedOnSize)

                    Divider().overlay(t.border)

                    actionStack(
                        requestId: pending.requestId,
                        suppressAlwaysAllowRule: pending.suppressAlwaysAllowRule,
                        autoModePrompt: pending.autoModePrompt
                    )
                    .padding(.horizontal, 20)
                    .padding(.vertical, 16)
                }
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .background(t.windowBg)
                .accessibilityElement(children: .contain)
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
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(t.text3)
            }
        }

        /// The monospaced, scrollable detail block. Permission decisions show
        /// every field in the request, after the same best-effort credential
        /// redaction used by the Desktop client.
        private func detailBlock(_ detail: String) -> some View {
            ScrollView {
                detailText(detail)
            }
            .scrollIndicators(.visible)
            .frame(height: 160)
            .background(t.surface, in: .rect(cornerRadius: 12))
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .stroke(t.border.opacity(0.72), lineWidth: 0.5)
            }
        }

        private func detailText(_ detail: String) -> some View {
            Text(detail)
                .font(.footnote.monospaced())
                .foregroundStyle(t.text2)
                .lineSpacing(4)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
        }

        /// iOS uses a vertical action hierarchy: the safest approval is the
        /// prominent action, broader grants are secondary, and denial remains a
        /// clearly destructive choice. Response mapping is unchanged.
        private func actionStack(
            requestId: UInt64,
            suppressAlwaysAllowRule: Bool,
            autoModePrompt: AutoModePromptDto?
        ) -> some View {
            VStack(spacing: 10) {
                promptButton(
                    String(localized: "permission_allow"),
                    prominent: true
                ) {
                    onApprove(requestId, .allowOnce)
                }
                if !suppressAlwaysAllowRule && autoModePrompt == nil {
                    promptButton(String(localized: "permission_allow_always")) {
                        onApprove(requestId, .allowAlways)
                    }
                }
                if !suppressAlwaysAllowRule, let autoModePrompt {
                    promptButton(autoModeApprovalLabel(autoModePrompt)) {
                        onApprove(requestId, .allowAuto)
                    }
                }
                promptButton(
                    String(localized: "permission_deny"),
                    role: .destructive
                ) {
                    onDeny(requestId)
                }
            }
        }

        private func autoModeApprovalLabel(_ prompt: AutoModePromptDto) -> String {
            switch prompt {
            case .workflowBash:
                return "Yes, and switch to auto mode"
            case .exitPlanMode:
                return "Yes, and use auto mode"
            }
        }

        @ViewBuilder
        private func promptButton(
            _ label: String,
            role: ButtonRole? = nil,
            prominent: Bool = false,
            action: @escaping () -> Void
        ) -> some View {
            let button = Button(role: role, action: action) {
                Text(label)
                    .font(.body.weight(.semibold))
                    .frame(maxWidth: .infinity)
            }
            .controlSize(.large)
            .tint(role == .destructive ? .red : t.accent)

            if prominent {
                button.buttonStyle(.borderedProminent)
            } else {
                button.buttonStyle(.bordered)
            }
        }

        // MARK: copy

        /// Human copy for each request kind, matching the Electron `describe`
        /// switch. `@unknown default` keeps a future kind renderable.
        static func describe(_ kind: PermissionKindDto) -> (
            title: String,
            summary: String,
            detailLabel: String,
            detailCaption: String,
            detail: String
        ) {
            switch kind {
            case let .toolUseConfirm(toolName, toolInputJson, _):
                return (
                    String(localized: "permission_allow_tool \(toolName)"),
                    "LingXi wants to use \(toolName). Review the requested input before continuing.",
                    "Requested action",
                    toolName,
                    permissionDetail(toolInputJson)
                )
            case let .exitPlanMode(plan):
                return (
                    String(localized: "permission_exit_plan_mode"),
                    "LingXi wants to leave plan mode and begin working from this plan.",
                    "Plan to execute",
                    "May run commands or modify files",
                    plan
                )
            case .bypassPermissionsMode:
                return (
                    String(localized: "permission_bypass_confirmation_title"),
                    "LingXi will be able to act without asking for further confirmation.",
                    "Permission scope",
                    "Commands, files, and connected services",
                    String(localized: "permission_bypass_confirmation_detail")
                )
            @unknown default:
                return (
                    String(localized: "permission_request_title"),
                    "LingXi needs your approval before it can continue.",
                    "Requested action",
                    "Current session",
                    ""
                )
            }
        }

        private static func permissionDetail(_ inputJson: String) -> String {
            guard !inputJson.isEmpty,
                  let data = inputJson.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data),
                  let input = object as? [String: Any],
                  !input.isEmpty
            else {
                return redactSensitiveText(inputJson)
            }

            let keys = input.keys.sorted { lhs, rhs in
                let dangerous = ["command", "code", "script"]
                let lhsIndex = dangerous.firstIndex(of: lhs) ?? dangerous.endIndex
                let rhsIndex = dangerous.firstIndex(of: rhs) ?? dangerous.endIndex
                return lhsIndex == rhsIndex ? lhs < rhs : lhsIndex < rhsIndex
            }
            if keys.count == 1, let value = input[keys[0]] {
                return redactSensitiveText(renderValue(value))
            }
            return redactSensitiveText(
                keys.compactMap { key in
                    guard let value = input[key] else { return nil }
                    return "\(key): \(renderValue(value))"
                }
                .joined(separator: "\n")
            )
        }

        private static func renderValue(_ value: Any) -> String {
            if let string = value as? String { return string }
            guard let data = try? JSONSerialization.data(
                      withJSONObject: value,
                      options: [.fragmentsAllowed, .sortedKeys, .prettyPrinted]
                  ),
                  let string = String(data: data, encoding: .utf8)
            else {
                return String(describing: value)
            }
            return string
        }

        private static func redactSensitiveText(_ value: String) -> String {
            var redacted = value
            redacted = redacted.replacingOccurrences(
                of: #"\b(sk-(?:ant-|proj-)?[A-Za-z0-9_-]{12,})\b"#,
                with: "[REDACTED]",
                options: .regularExpression
            )
            redacted = redacted.replacingOccurrences(
                of: #"\b(Bearer\s+)[A-Za-z0-9._~+/-]+=*"#,
                with: "$1[REDACTED]",
                options: [.regularExpression, .caseInsensitive]
            )
            return redacted.replacingOccurrences(
                of: #"((?:api[_-]?key|token|secret|password)\s*[=:]\s*)[^\s,;]+"#,
                with: "$1[REDACTED]",
                options: [.regularExpression, .caseInsensitive]
            )
        }

        private static func riskCopy(for pending: PendingPermission) -> String {
            if isElevatedRisk(pending) {
                return "This can expose or modify sensitive data. Continue only if you trust the current session."
            }
            if pending.autoModePrompt != nil, !pending.suppressAlwaysAllowRule {
                return "Auto mode can approve future actions without asking. Review the requested scope carefully."
            }
            if !pending.suppressAlwaysAllowRule, pending.autoModePrompt == nil {
                return "Allow matching actions saves a rule for this workspace. Use it only when you trust future matching requests."
            }
            return "This decision applies only to the current request."
        }

        private static func isElevatedRisk(_ pending: PendingPermission) -> Bool {
            if case .bypassPermissionsMode = pending.kind { return true }
            return false
        }
    }

#endif
