import Foundation
import XCTest
@testable import LingxiCode

/// External-browser transport acceptance. A new browser request must be held
/// until the native iOS 26 foreground action has attached its real lease.
@MainActor
final class LocalAppAgentAcceptanceTests: XCTestCase {
    private func publishAgentApp(_ client: LocalAppServiceClient) async throws -> (LocalAppPublishResult, URL) {
        let paths = MobileUserStorage.directories()
        let guest = paths.appsGuestPath + "/agent-wake-acceptance-" + UUID().uuidString.lowercased()
        let source = try UnifiedLocalApps.hostSourceDirectory(guest)
        let fixtures = try XCTUnwrap(Bundle(for: Self.self).resourceURL).appendingPathComponent("UnifiedAppFixtures/agent")
        try FileManager.default.createDirectory(at: source.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.copyItem(at: fixtures, to: source)
        do {
            let result: LocalAppPublishResult = try await client.request([
                "op": .string("publish"), "sourcePath": .string(source.path), "expectedRevision": .number(0)
            ])
            return (result, source)
        } catch {
            try? FileManager.default.removeItem(at: source)
            throw error
        }
    }

    func testBrowserRunAgentRequiresAttachedNativeLeaseOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("The Local App loopback service and native task lease require a physical iPhone.")
        #else
        let client = LocalAppServiceClient()
        let app = try await publishAgentApp(client)
        do {
            let launch = try await UnifiedLocalApps(client: client).openExternalBrowser(app.0.record.appId)
            XCTAssertTrue(LocalAppExternalBrowserURL.matches(launch.url, origin: launch.origin))

            let boundary = "LingxiAgentTest-\(UUID().uuidString)"
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
            var body = Data()
            func append(_ text: String) { body.append(Data(text.utf8)) }
            append("--\(boundary)\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n")
            body.append(metadata)
            append("\r\n--\(boundary)--\r\n")

            let endpoint = try XCTUnwrap(URL(string: launch.origin + "/__run/\(launch.run.runId)/__host/agent"))
            var request = URLRequest(url: endpoint)
            request.httpMethod = "POST"
            request.timeoutInterval = 30
            request.setValue(launch.origin, forHTTPHeaderField: "Origin")
            request.setValue(launch.url.absoluteString, forHTTPHeaderField: "Referer")
            request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
            request.setValue("1", forHTTPHeaderField: "X-LingXi-App-Request")
            request.httpBody = body
            let (bytes, response) = try await URLSession.shared.data(for: request)
            XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
            let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: bytes) as? [String: Any])
            XCTAssertEqual(envelope["ok"] as? Bool, false)
            let error = try XCTUnwrap(envelope["error"] as? [String: Any])
            XCTAssertEqual(error["code"] as? String, "host_wake_required")
            print("IOS_AGENT_WAKE_GATE PASS external-gateway tasks.start RunAgentInput ContentPart-data runId no-native-lease")
        } catch {
            try? await UnifiedLocalApps(client: client).action("remove", id: app.0.record.appId)
            try? FileManager.default.removeItem(at: app.1)
            throw error
        }
        try? await UnifiedLocalApps(client: client).action("remove", id: app.0.record.appId)
        try? FileManager.default.removeItem(at: app.1)
        #endif
    }

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
