import Foundation
import XCTest
@testable import LingxiCode

/// External-browser transport acceptance. A new browser request must be held
/// until the native iOS 26 foreground action has attached its real lease.
@MainActor
final class LocalAppAgentAcceptanceTests: XCTestCase {
#if canImport(harness_runtimeFFI)
    @MainActor
    private func publishAgentApp(_ client: LocalAppServiceClient) async throws -> (LocalAppPublishResult, URL, String) {
        let paths = MobileUserStorage.directories()
        let guest = paths.appsGuestPath + "/agent-wake-acceptance-" + UUID().uuidString.lowercased()
        let source = try UnifiedLocalApps.hostSourceDirectory(guest)
        let fixtures = try XCTUnwrap(Bundle(for: Self.self).resourceURL).appendingPathComponent("UnifiedAppFixtures/agent")
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
            let project = try await projectStore.createLocalAppAuthoringProject(name: "Agent wake acceptance", sourceDirectory: guest)
            projectID = project.record.id
            stage = "create-engine-root"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try FileManager.default.createDirectory(at: engineRoot, withIntermediateDirectories: true)
            let runtimeMetadata = LXISHRuntimeBundleMetadata.current()
            let workspaceHostPath = try MobileUserStorage.prepareProjectsDirectory().path
            let mobileLinux = TerminalRuntimeConfig(
                mode: .mobileLinux,
                managedRoot: LXISHDefaultWorkspace.managedRootPath(),
                workspaceHostPath: workspaceHostPath,
                stableWorkspaceId: LXISHDefaultWorkspace.stableID(),
                abi: "arm64",
                rootfsVersion: runtimeMetadata.rootfsVersion,
                archiveSha256: runtimeMetadata.archiveSha256,
                authorizationFile: LXISHRuntimeBundleResources.authorizationManifestURL()?.path
            )
            let projectMountGrants = try MobileUserStorage.projectMountGrantPaths(for: project)
            // Register an actual Code session for this dedicated source Project
            // before the test exercises the external-browser wake gate.
            stage = "start-code-session"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let conversation = ConversationSourceFactory.make(
                projectCwd: project.workspace.hostURL.path,
                additionalWorkspaceDirectories: project.record.additionalDirectories,
                sessionMode: .code,
                providerConfigured: false,
                providerProfilesJson: nil,
                providerRoutingJson: nil,
                visionDelegationEnabled: false,
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
            stage = "record-code-session"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try await projectStore.recordStartedSession(
                projectId: project.record.id,
                sessionId: sessionID,
                title: "Agent wake acceptance",
                mode: .code
            )
            stage = "record-workspace"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            try await projectStore.recordWorkspace(projectId: project.record.id, state: workspace)
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
            return (result, source, project.record.id)
        } catch {
            print("IOS_AGENT_WAKE_FAILURE stage=\(stage) error=\(String(describing: error))")
            engine = nil
            if let projectID { _ = try? projectRepository.deleteProject(projectId: projectID) }
            try? FileManager.default.removeItem(at: source)
            try? FileManager.default.removeItem(at: engineRoot)
            if error is CancellationError {
                throw LocalAppServiceFailure("TEST_SETUP_CANCELLED", "Agent wake acceptance canceled during \(stage).")
            }
            throw error
        }
    }

    private func configureAgentForWakeGate(_ client: LocalAppServiceClient, appId: String) async throws {
        print("IOS_AGENT_WAKE_CHECKPOINT catalog")
        let catalog = try await client.agentHost(["op": .string("catalog")], as: LocalAppJSON.self)
        guard case .object(let fields) = catalog, case .array(let models)? = fields["models"],
              let model = models.first(where: { value in
                  guard case .object(let modelFields) = value else { return false }
                  return modelFields["canSelect"] == .bool(true)
              }),
              case .object(let modelFields) = model,
              case .string(let providerId)? = modelFields["providerId"],
              case .string(let modelId)? = modelFields["modelId"] else {
            throw XCTSkip("Configure a selectable model in the native host to exercise the iOS wake gate.")
        }
        let settings = LocalAppJSON.object([
            "model": .object(["providerId": .string(providerId), "modelId": .string(modelId)]),
            "tools": .object([:]),
            "skills": .array([]),
            "background": .bool(false),
            "maxTurns": .number(2),
            "timeoutMs": .number(120_000),
        ])
        print("IOS_AGENT_WAKE_CHECKPOINT configure")
        let _: LocalAppJSON = try await client.agentHost([
            "op": .string("configure"),
            "appId": .string(appId),
            "settings": settings,
        ], as: LocalAppJSON.self)
    }

    func testBrowserRunAgentRequiresAttachedNativeLeaseOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("The Local App loopback service and native task lease require a physical iPhone.")
        #else
        let client = LocalAppServiceClient()
        let app = try await publishAgentApp(client)
        var stage = "configure"
        do {
            try await configureAgentForWakeGate(client, appId: app.0.record.appId)
            stage = "open-browser"
            print("IOS_AGENT_WAKE_CHECKPOINT \(stage)")
            let launch = try await UnifiedLocalApps(client: client).openExternalBrowser(app.0.record.appId)
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
            print("IOS_AGENT_WAKE_FAILURE stage=\(stage) error=\(String(describing: error))")
            try? await UnifiedLocalApps(client: client).action("remove", id: app.0.record.appId)
            try? FileManager.default.removeItem(at: app.1)
            _ = try? ProjectRepository(projectsRoot: URL(fileURLWithPath: MobileUserStorage.directories().projects, isDirectory: true))
                .deleteProject(projectId: app.2)
            if error is CancellationError {
                throw LocalAppServiceFailure("TEST_SETUP_CANCELLED", "Agent wake acceptance canceled during \(stage).")
            }
            throw error
        }
        try? await UnifiedLocalApps(client: client).action("remove", id: app.0.record.appId)
        try? FileManager.default.removeItem(at: app.1)
        _ = try? ProjectRepository(projectsRoot: URL(fileURLWithPath: MobileUserStorage.directories().projects, isDirectory: true))
            .deleteProject(projectId: app.2)
        #endif
    }
#endif

    func testStageCalorieAppForExternalBrowserAcceptance() async throws {
        let client = LocalAppServiceClient()
        let guest = MobileUserStorage.directories().appsGuestPath + "/browser-meal-" + UUID().uuidString.lowercased()
        let source = try UnifiedLocalApps.hostSourceDirectory(guest)
        let fixture = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "UnifiedAppFixtures", withExtension: nil))
        try FileManager.default.copyItem(at: fixture.appendingPathComponent("calorie-agent"), to: source)
        let app: LocalAppPublishResult = try await client.request([
            "op": .string("publish"), "sourcePath": .string(source.path), "expectedRevision": .number(0)
        ])
        let receipt = try JSONSerialization.data(withJSONObject: ["appId": app.record.appId, "sourcePath": source.path], options: [.prettyPrinted, .sortedKeys])
        let attachment = XCTAttachment(data: receipt, uniformTypeIdentifier: "public.json")
        attachment.name = "external-browser-interactive-app"
        attachment.lifetime = .keepAlways
        add(attachment)
        try receipt.write(to: URL(fileURLWithPath: MobileUserStorage.directories().configHome)
            .appendingPathComponent("external-browser-interactive-acceptance.json"))
        print("IOS_BROWSER_INTERACTIVE appId=\(app.record.appId)")
    }
}
