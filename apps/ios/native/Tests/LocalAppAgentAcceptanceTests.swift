import Foundation
import XCTest
@testable import LingxiCode

/// External-browser transport acceptance. A new browser request must be held
/// until the native iOS 26 foreground action has attached its real lease.
@MainActor
final class LocalAppAgentAcceptanceTests: XCTestCase {
#if canImport(harness_runtimeFFI)
    private struct PublishedAgentApp {
        let result: LocalAppPublishResult
        let source: URL
        let projectID: String
        let sessionID: String
    }

    @MainActor
    private func publishAgentApp(
        _ client: LocalAppServiceClient,
        fixtureName: String = "agent",
        projectName: String = "Agent wake acceptance",
        verifyProjectShell: Bool = true
    ) async throws -> PublishedAgentApp {
        let paths = MobileUserStorage.directories()
        let providerSnapshot = ProviderRepository.shared.makeLaunchSnapshot()
        guard !providerSnapshot.enabledProfileIDs.isEmpty else {
            throw XCTSkip("Configure a native model before exercising the iOS Project source mount.")
        }
        let codeModel = try await configuredAgentModel(client, requireImages: false, requireTools: true, requireStructuredOutput: false)
        guard let codeProfile = providerSnapshot.profiles.first(where: {
            $0.enabled && $0.id == codeModel.providerId && $0.modelID == codeModel.modelId
        }) else {
            throw LocalAppServiceFailure("MODEL_PROFILE_MISMATCH", "The tool-capable Agent catalog model did not match an enabled native ProviderLaunchProfile.")
        }
        let codeModelID = codeProfile.qualifiedModelID
        print("IOS_AGENT_CODE_MODEL providerId=\(codeModel.providerId) modelId=\(codeModel.modelId) qualifiedModelId=\(codeModelID) supportsTools=true selectedFrom=ProviderLaunchSnapshot")
        let guest = paths.appsGuestPath + "/agent-wake-acceptance-" + UUID().uuidString.lowercased()
        let source = try UnifiedLocalApps.hostSourceDirectory(guest)
        let fixtures = try XCTUnwrap(Bundle(for: Self.self).resourceURL).appendingPathComponent("UnifiedAppFixtures/\(fixtureName)")
        let projectRepository = ProjectRepository(
            projectsRoot: URL(fileURLWithPath: paths.projects, isDirectory: true),
            userHome: URL(fileURLWithPath: paths.home, isDirectory: true)
        )
        let projectStore = ProjectStore(
            repository: projectRepository,
            synchronizer: ProjectWorkspaceSynchronizer(repository: projectRepository)
        )
        let engineRoot = FileManager.default.temporaryDirectory.appendingPathComponent("ios-agent-wake-engine-\(UUID().uuidString)", isDirectory: true)
        var projectID: String?
        var stage = "create-source"
        var engine: EngineConversationSource?
        do {
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try FileManager.default.createDirectory(at: source.deletingLastPathComponent(), withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: fixtures, to: source)
            stage = "create-project"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let project = try await projectStore.createLocalAppAuthoringProject(name: projectName, sourceDirectory: guest)
            projectID = project.record.id
            let projectMountGrants = try MobileUserStorage.projectMountGrantPaths(for: project)
            print("IOS_AGENT_PROJECT_SETUP projectId=\(project.record.id) guestSource=\(guest) activeDirectory=\(project.record.activeDirectory) additionalDirectories=\(project.record.additionalDirectories.sorted()) workspaceHost=\(project.workspace.hostURL.path) sourceHost=\(source.path) projectMountGrants=\(projectMountGrants.sorted())")
            guard project.record.activeDirectory == guest,
                  project.record.additionalDirectories.contains(guest),
                  projectMountGrants.contains(guest) else {
                throw LocalAppServiceFailure("PROJECT_SETUP_MISMATCH", "The native Project did not register the exact Local App source directory as its active, additional, and granted path.")
            }
            stage = "create-engine-root"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try FileManager.default.createDirectory(at: engineRoot, withIntermediateDirectories: true)
            let runtime = TerminalRuntimeDescriptor.make(
                appSandboxRoot: ConversationSourceFactory.appSandboxRoot(),
                project: project,
                linuxRuntime: LinuxRuntimeState()
            )
            guard let mobileLinux = runtime.config else {
                throw LocalAppServiceFailure("PROJECT_RUNTIME_UNAVAILABLE", "The production Mobile Linux runtime descriptor could not configure this Project.")
            }
            print("IOS_AGENT_RUNTIME_CONFIG managedRoot=\(mobileLinux.managedRoot) workspaceHost=\(mobileLinux.workspaceHostPath) stableWorkspaceId=\(mobileLinux.stableWorkspaceId) descriptorWorkspaceHost=\(runtime.workspace.hostPath) descriptorWorkspaceGuest=\(runtime.workspace.guestPath)")
            // Register an actual Code session for this dedicated source Project
            // before the test exercises the external-browser wake gate.
            stage = "start-code-session"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let conversation = ConversationSourceFactory.make(
                projectCwd: project.workspace.hostURL.path,
                additionalWorkspaceDirectories: project.record.additionalDirectories,
                sessionMode: .code,
                providerConfigured: true,
                providerProfilesJson: providerSnapshot.providerProfilesJSON,
                providerRoutingJson: providerSnapshot.routingJSON,
                defaultModelID: codeModelID,
                visionDelegationEnabled: providerSnapshot.visionDelegationEnabled,
                mobileLinux: mobileLinux,
                projectMountGrants: projectMountGrants
            )
            engine = conversation as? EngineConversationSource
            guard let activeEngine = engine else {
                throw LocalAppServiceFailure("ENGINE_UNAVAILABLE", "The production conversation factory did not create the engine.")
            }
            let sessionID = try await activeEngine.startLocalAppAuthoringSession(cwd: guest)
            stage = "verify-code-session"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            guard let workspace = activeEngine.model.workspaceState,
                  workspace.sessionId == sessionID,
                  workspace.cwd == guest else {
                throw LocalAppServiceFailure("AUTHORING_CHANGED", "The test Code session did not confirm the app source directory.")
            }
            print("IOS_AGENT_CODE_SESSION sessionId=\(sessionID) activeSessionId=\(activeEngine.model.activeSessionId) cwd=\(workspace.cwd) projectRoot=\(workspace.projectRoot) additionalDirectories=\(workspace.additionalDirectories.sorted())")
            stage = "record-code-session"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try await projectStore.recordStartedSession(
                projectId: project.record.id,
                sessionId: sessionID,
                title: projectName,
                mode: .code
            )
            stage = "record-workspace"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try await projectStore.recordWorkspace(projectId: project.record.id, state: workspace)
            if verifyProjectShell {
                stage = "verify-project-shell-cwd"
                print("IOS_AGENT_WAKE_CHECKPOINT \(stage) model=\(codeModelID)")
                try await verifyProjectMountWithConfiguredShell(
                    activeEngine,
                    guestSource: guest,
                    hostSource: source
                )
            }
            let authoring = LocalAppAuthoring(
                sourceDirectory: guest,
                workspaceId: project.record.id,
                projectRef: .mobileProject(project.record.id),
                authoringSessionId: sessionID
            )
            stage = "register-pending-authoring"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try await UnifiedLocalApps(client: client).registerPendingAuthoring(authoring)
            stage = "publish"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let result: LocalAppPublishResult = try await client.request([
                "op": .string("publish"), "sourcePath": .string(source.path), "expectedRevision": .number(0)
            ])
            engine = nil
            try? FileManager.default.removeItem(at: engineRoot)
            return PublishedAgentApp(result: result, source: source, projectID: project.record.id, sessionID: sessionID)
        } catch {
            print("IOS_AGENT_PUBLISH_FAILURE stage=\(stage) type=\(String(reflecting: type(of: error))) error=\(String(describing: error))")
            engine = nil
            if let projectID { _ = try? projectRepository.deleteProject(projectId: projectID) }
            try? FileManager.default.removeItem(at: source)
            try? FileManager.default.removeItem(at: engineRoot)
            throw error
        }
    }

    /// Exercises the native Code session's normal model/tool path. The one-time
    /// approval below is narrowly bound to the exact read-only probe command;
    /// no permission mode or sandbox policy is changed for the session.
    @MainActor
    private func verifyProjectMountWithConfiguredShell(
        _ engine: EngineConversationSource,
        guestSource: String,
        hostSource: URL
    ) async throws {
        let markerName = ".ios-project-mount-\(UUID().uuidString.lowercased()).txt"
        let marker = "IOS_PROJECT_SOURCE_MARKER_\(UUID().uuidString)"
        let markerURL = hostSource.appendingPathComponent(markerName)
        try Data(marker.utf8).write(to: markerURL, options: .atomic)
        defer { try? FileManager.default.removeItem(at: markerURL) }

        let command = "pwd && cat ./\(markerName)"
        let prompt = "Use the Bash tool once with this exact command: `\(command)`. Do not infer the output."
        guard let token = engine.send(prompt) else {
            throw LocalAppServiceFailure("PROJECT_SHELL_NOT_SUBMITTED", "The Code session did not accept the Project mount probe.")
        }

        let deadline = Date().addingTimeInterval(120)
        while Date() < deadline {
            try Task.checkCancellation()
            if let pending = engine.model.pendingPermissions.first {
                guard case let .toolUseConfirm(toolName, toolInputJson, _) = pending.kind,
                      ["bash", "shell"].contains(toolName.lowercased()),
                      let json = toolInputJson.data(using: .utf8),
                      let input = try? JSONSerialization.jsonObject(with: json) as? [String: Any],
                      input["command"] as? String == command,
                      (input["dangerouslyDisableSandbox"] as? Bool ?? false) == false else {
                    engine.denyPermission(pending.requestId)
                    try? await engine.cancelAndWait()
                    throw LocalAppServiceFailure("PROJECT_SHELL_PERMISSION_MISMATCH", "The model requested a command outside the exact read-only Project mount probe.")
                }
                engine.approvePermission(pending.requestId, .allowOnce)
            }

            if let completion = engine.model.turnCompletion, completion.token == token {
                let run = engine.model.items.reversed().compactMap { item -> ConversationExecutionRun? in
                    guard case let .run(run) = item, run.turnId == token.clientTurnId else { return nil }
                    return run
                }.first
                let shellCards = run?.shellCards ?? []
                let shell = shellCards.first { $0.command == command } ?? shellCards.first
                let shellDiagnostics = shell.map {
                    "command=\($0.command.debugDescription) status=\($0.status.label) cwd=\($0.cwd ?? "<none>") exitCode=\($0.exitCode.map { String($0) } ?? "<none>") truncated=\($0.truncated) stdout=\(String($0.stdout.prefix(1500)).debugDescription) stderr=\(String($0.stderr.prefix(1500)).debugDescription)"
                } ?? "no-shell-card toolCount=\(run?.tools.count ?? 0)"
                if completion.outcome != .completed {
                    print("IOS_AGENT_WAKE_PROJECT_MOUNT_RESULT outcome=\(String(describing: completion.outcome)) \(shellDiagnostics)")
                    throw LocalAppServiceFailure("PROJECT_SHELL_TURN_FAILED", "The Code session's Project mount probe did not complete; \(shellDiagnostics)")
                }
                guard let shell else {
                    print("IOS_AGENT_WAKE_PROJECT_MOUNT_RESULT turnId=\(token.clientTurnId) \(shellDiagnostics)")
                    throw LocalAppServiceFailure("PROJECT_SHELL_RESULT_MISSING", "The completed Code turn did not report a Bash tool result; \(shellDiagnostics)")
                }
                print("IOS_AGENT_WAKE_PROJECT_MOUNT_RESULT \(shellDiagnostics)")
                guard shell.command == command, shell.status == .completed, shell.exitCode == 0,
                      shell.stdout.contains(guestSource), shell.stdout.contains(marker) else {
                    throw LocalAppServiceFailure("PROJECT_SHELL_RESULT_MISMATCH", "Bash did not confirm the exact Project source probe; expectedCommand=\(command.debugDescription) \(shellDiagnostics)")
                }
                print("IOS_AGENT_WAKE_PROJECT_MOUNT PASS guest=\(guestSource) \(shellDiagnostics)")
                return
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        try? await engine.cancelAndWait()
        throw LocalAppServiceFailure("PROJECT_SHELL_TIMEOUT", "The Code session did not finish the Project mount probe within 120 seconds.")
    }

    private func configuredAgentModel(
        _ client: LocalAppServiceClient,
        requireImages: Bool,
        requireTools: Bool,
        requireStructuredOutput: Bool
    ) async throws -> (providerId: String, modelId: String) {
        print("IOS_AGENT_WAKE_CHECKPOINT catalog")
        let catalog = try await client.agentHost(["op": .string("catalog")], as: LocalAppJSON.self)
        guard case .object(let fields) = catalog, case .array(let models)? = fields["models"],
              let model = models.first(where: { value in
                  guard case .object(let modelFields) = value else { return false }
                  return modelFields["canSelect"] == .bool(true)
                      && (!requireImages || modelFields["supportsImages"] == .bool(true))
                      && (!requireTools || modelFields["supportsTools"] == .bool(true))
                      && (!requireStructuredOutput || modelFields["supportsStructuredOutput"] == .bool(true))
              }),
              case .object(let modelFields) = model,
              case .string(let providerId)? = modelFields["providerId"],
              case .string(let modelId)? = modelFields["modelId"] else {
            let requirement = [
                requireImages ? "image" : nil,
                requireTools ? "tool" : nil,
                requireStructuredOutput ? "structured-output" : nil,
            ].compactMap { $0 }.joined(separator: " and ")
            throw XCTSkip("Configure an available native model with \(requirement) support to exercise the iOS acceptance.")
        }
        return (providerId, modelId)
    }

    private func configureAgentForWakeGate(_ client: LocalAppServiceClient, appId: String) async throws -> String {
        let selectedModel = try await configuredAgentModel(
            client,
            requireImages: true,
            requireTools: false,
            requireStructuredOutput: true
        )
        let settings = LocalAppJSON.object([
            "model": .object(["providerId": .string(selectedModel.providerId), "modelId": .string(selectedModel.modelId)]),
            "tools": .object([:]),
            "skills": .array([]),
            "background": .bool(false),
            "maxTurns": .number(8),
            "timeoutMs": .number(180_000),
        ])
        print("IOS_AGENT_WAKE_CHECKPOINT configure")
        let _: LocalAppJSON = try await client.agentHost([
            "op": .string("configure"),
            "appId": .string(appId),
            "settings": settings,
        ], as: LocalAppJSON.self)
        print("IOS_AGENT_WAKE_MODEL providerId=\(selectedModel.providerId) modelId=\(selectedModel.modelId) supportsImages=true supportsStructuredOutput=true")
        return "\(selectedModel.providerId)/\(selectedModel.modelId)"
    }

    func testBrowserRunAgentRequiresAttachedNativeLeaseOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("The Local App loopback service and native task lease require a physical iPhone.")
        #else
        let client = LocalAppServiceClient()
        let app = try await publishAgentApp(client)
        var stage = "configure"
        do {
            _ = try await configureAgentForWakeGate(client, appId: app.result.record.appId)
            stage = "open-browser"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let launch = try await UnifiedLocalApps(client: client).openExternalBrowser(app.result.record.appId)
            XCTAssertTrue(LocalAppExternalBrowserURL.matches(launch.url, origin: launch.origin))

            let runID = "ios opaque run + \(UUID().uuidString)"
            let attachment = Data("small public test attachment".utf8)
            let runInput: [String: Any] = [
                "threadId": "thread-\(UUID().uuidString)",
                "runId": runID,
                "messages": [[
                    "id": "message-\(UUID().uuidString)",
                    "role": "user",
                    "content": [
                        ["type": "text", "text": "Read the attached note and summarize it."],
                        ["type": "document", "source": [
                            "type": "data",
                            "value": attachment.base64EncodedString(),
                            "mimeType": "text/plain"
                        ], "metadata": ["com.lingxi.localApp": ["version": 1, "filename": "note.txt"]]]
                    ]
                ]],
                "forwardedProps": ["com.lingxi.localApp": ["version": 1, "tools": []]]
            ]
            let metadata = try JSONSerialization.data(withJSONObject: [
                "method": "agent.tasks.start",
                "args": ["input": runInput]
            ], options: [.sortedKeys])

            let endpoint = try XCTUnwrap(URL(string: launch.origin + "/__run/\(launch.run.runId)/__host/agent"))
            var request = URLRequest(url: endpoint)
            request.httpMethod = "POST"
            request.timeoutInterval = 30
            request.setValue(launch.origin, forHTTPHeaderField: "Origin")
            request.setValue(launch.url.absoluteString, forHTTPHeaderField: "Referer")
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.setValue("1", forHTTPHeaderField: "X-LingXi-App-Request")
            request.httpBody = metadata
            stage = "post-run-input"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let (bytes, response) = try await URLSession.shared.data(for: request)
            XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
            let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: bytes) as? [String: Any])
            XCTAssertEqual(envelope["ok"] as? Bool, false)
            let error = try XCTUnwrap(envelope["error"] as? [String: Any])
            XCTAssertEqual(error["code"] as? String, "host_wake_required")
            print("IOS_AGENT_WAKE_GATE PASS source Project and Code session; JSON RunAgentInput ContentPart-data; no native lease")
        } catch {
            print("IOS_AGENT_WAKE_FAILURE stage=\(stage) type=\(String(reflecting: type(of: error))) error=\(String(describing: error))")
            try? await UnifiedLocalApps(client: client).action("remove", id: app.result.record.appId)
            try? FileManager.default.removeItem(at: app.source)
            _ = try? ProjectRepository(projectsRoot: URL(fileURLWithPath: MobileUserStorage.directories().projects, isDirectory: true))
                .deleteProject(projectId: app.projectID)
            throw error
        }
        try? await UnifiedLocalApps(client: client).action("remove", id: app.result.record.appId)
        try? FileManager.default.removeItem(at: app.source)
        _ = try? ProjectRepository(projectsRoot: URL(fileURLWithPath: MobileUserStorage.directories().projects, isDirectory: true))
            .deleteProject(projectId: app.projectID)
        #endif
    }
#endif

    func testStageCalorieAppForExternalBrowserAcceptance() async throws {
#if canImport(harness_runtimeFFI)
        let client = LocalAppServiceClient()
        let app = try await publishAgentApp(
            client,
            fixtureName: "calorie-agent",
            projectName: "Calorie browser acceptance",
            verifyProjectShell: false
        )
        do {
            let model = try await configureAgentForWakeGate(client, appId: app.result.record.appId)
            try attachBrowserAcceptanceReceipt(app, fixtureName: "calorie-agent", model: model)
        } catch {
            try? await UnifiedLocalApps(client: client).action("remove", id: app.result.record.appId)
            try? FileManager.default.removeItem(at: app.source)
            _ = try? ProjectRepository(projectsRoot: URL(fileURLWithPath: MobileUserStorage.directories().projects, isDirectory: true))
                .deleteProject(projectId: app.projectID)
            throw error
        }
#else
        throw XCTSkip("FullDebug Mobile Linux is required to stage a real Project-backed Local App.")
#endif
    }

    func testStageAGUIBrowserAppForExternalBrowserAcceptance() async throws {
#if canImport(harness_runtimeFFI)
        let client = LocalAppServiceClient()
        let app = try await publishAgentApp(
            client,
            fixtureName: "agent",
            projectName: "AG-UI browser acceptance",
            verifyProjectShell: false
        )
        do {
            let model = try await configureAgentForWakeGate(client, appId: app.result.record.appId)
            let launch = try await UnifiedLocalApps(client: client).openExternalBrowser(app.result.record.appId)
            XCTAssertTrue(LocalAppExternalBrowserURL.matches(launch.url, origin: launch.origin))
            try attachBrowserAcceptanceReceipt(app, fixtureName: "agent", model: model)
            print("IOS_AGUI_EXTERNAL_BROWSER_OPEN appId=\(app.result.record.appId) projectId=\(app.projectID) route=system-selected-external-browser")
        } catch {
            try? await UnifiedLocalApps(client: client).action("remove", id: app.result.record.appId)
            try? FileManager.default.removeItem(at: app.source)
            _ = try? ProjectRepository(projectsRoot: URL(fileURLWithPath: MobileUserStorage.directories().projects, isDirectory: true))
                .deleteProject(projectId: app.projectID)
            throw error
        }
#else
        throw XCTSkip("FullDebug Mobile Linux is required to stage a real Project-backed Local App.")
#endif
    }

#if canImport(harness_runtimeFFI)
    private func attachBrowserAcceptanceReceipt(_ app: PublishedAgentApp, fixtureName: String, model: String) throws {
        let appId = app.result.record.appId
        let receiptObject: [String: Any] = [
            "appId": appId,
            "revision": NSNumber(value: app.result.record.currentRevision),
            "fixture": fixtureName,
            "sourcePath": app.source.path,
            "projectId": app.projectID,
            "authoringSessionId": app.sessionID,
            "model": model,
        ]
        let receipt = try JSONSerialization.data(withJSONObject: receiptObject, options: [.prettyPrinted, .sortedKeys])
        let attachment = XCTAttachment(data: receipt, uniformTypeIdentifier: "public.json")
        attachment.name = "agui-browser-acceptance-\(appId)"
        attachment.lifetime = .keepAlways
        add(attachment)
        let receiptURL = URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
            .appendingPathComponent("agui-browser-acceptance-\(appId).json")
        try receipt.write(to: receiptURL, options: .atomic)
        print("IOS_BROWSER_ACCEPTANCE_STAGED appId=\(appId) projectId=\(app.projectID) fixture=\(fixtureName) model=\(model) receipt=\(receiptURL.path)")
    }
#endif
}
