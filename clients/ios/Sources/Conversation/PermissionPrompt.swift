// PermissionPrompt.swift — SHIP-BLOCKER #3.
//
// The SwiftUI allow/deny surface for engine-parked permission requests, the iOS
// mirror of the Electron renderer's `PermissionPrompt.tsx` (7.3).
//
// The engine's adapter permission gate emits a `PermissionRequest` whenever a
// tool needs approval (e.g. a Write/Bash invocation) and parks the turn on a
// oneshot until the user answers. `EnginePermissionSink` forwards each request
// onto `ConversationModel.pendingPermissions`; the app-level host presents the
// head above the current UIKit surface with three actions — Deny / Allow always /
// Allow once — each resolving
// the park by submitting `ClientCommand.approvePermission` / `denyPermission`
// (correlated by `requestId`) back through the `MobileEngineHandle`.
//
// Compiled only when the engine bindings are linked: the request DTOs are
// UniFFI types, and the mock source never parks a turn on a permission gate.

import SwiftUI

#if canImport(engine_mobileFFI)

    enum PermissionPromptPresentationPolicy {
        static func canPresent(
            sceneActivationState: UIScene.ActivationState,
            presenterIsAttached: Bool
        ) -> Bool {
            sceneActivationState == .foregroundActive && presenterIsAttached
        }
    }

    /// Observes the engine-scoped queue and presents its head above the currently
    /// visible UIKit controller. A separate over-full-screen presentation is used
    /// because a SwiftUI overlay attached to the chat remains behind sheets and
    /// full-screen covers.
    struct EnginePermissionPromptHost: View {
        @ObservedObject var model: ConversationModel
        @Environment(\.theme) private var theme
        @Environment(\.locale) private var locale

        let onApprove: (UInt64, PermissionResponseDto) -> Void
        let onDeny: (UInt64) -> Void

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
                host.modalPresentationStyle = .overFullScreen
                host.modalTransitionStyle = .crossDissolve

                presentationGeneration &+= 1
                let generation = presentationGeneration
                hostingController = host
                presentationInFlight = true
                present(host, from: presenter, generation: generation)
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
                if let fallback,
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
                .accessibilityAddTraits(.isModal)
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
