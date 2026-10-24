import Foundation
import UIKit
import WebKit
import XCTest
@testable import LingxiCode

/// Real configured provider, signed physical-device host and actual WK page SDK.
/// Never installs credentials or substitutes model output. Only own apps are removed.
@MainActor
final class LocalAppAgentAcceptanceTests: XCTestCase {
    private func json(_ value: Any) throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), as: UTF8.self)
    }
    private func object(_ value: LocalAppJSON) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? [String: Any])
    }
    private func host(_ client: LocalAppServiceClient, _ value: [String: Any]) async throws -> [String: Any] {
        try object(await client.agentHost(json(value)))
    }
    private func page(_ client: LocalAppServiceClient, _ launch: LocalAppLaunch, _ method: String, _ value: [String: Any]) async throws -> [String: Any] {
        try object(await client.agentPage(appID: launch.run.appId, invocationID: launch.run.runId, method: method, arguments: json(value)))
    }
    private func publish(_ client: LocalAppServiceClient) async throws -> (LocalAppPublishResult, URL) {
        let guest = "/home/lingxi/.lingxi/.apps/agent-acceptance-" + UUID().uuidString
        let source = try UnifiedLocalApps.hostSourceDirectory(guest)
        let fixtures = try XCTUnwrap(Bundle(for: Self.self).resourceURL).appendingPathComponent("UnifiedAppFixtures/agent")
        try FileManager.default.createDirectory(at: source.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.copyItem(at: fixtures, to: source)
        do {
            let result: LocalAppPublishResult = try await client.request([
                "op": .string("publish"), "sourcePath": .string(source.path), "expectedRevision": .number(0)])
            return (result, source)
        } catch { try? FileManager.default.removeItem(at: source); throw error }
    }
    private func configure(_ client: LocalAppServiceClient, _ id: String) async throws {
        let catalog = try await host(client, ["op": "catalog"])
        let environment = ProcessInfo.processInfo.environment
        let provider = environment["LINGXI_AGENT_PROVIDER"] ?? "deepseek"
        let model = environment["LINGXI_AGENT_MODEL"] ?? "deepseek-flash"
        let models = try XCTUnwrap(catalog["models"] as? [[String: Any]])
        XCTAssertTrue(models.contains { ($0["providerId"] as? String) == provider && ($0["modelId"] as? String) == model && ($0["canSelect"] as? Bool) == true && ($0["supportsImages"] as? Bool) == true && ($0["supportsStructuredOutput"] as? Bool) == true }, "Configure the explicitly selected vision/tool model on this device first")
        _ = try await client.agentHost(json(["op": "configure", "appId": id, "settings": [
            "model": ["providerId": provider, "modelId": model], "tools": [:] as [String: Any],
            "background": false, "maxTurns": 8, "timeoutMs": 180000]]))
    }
    private func wait(_ view: WKWebView, _ condition: String, seconds: TimeInterval = 25) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if try await view.evaluateJavaScript(condition) as? Bool == true { return }
            let error = try await view.evaluateJavaScript("window.__agentFailure || null")
            if let message = error as? String { throw LocalAppServiceFailure("ACCEPTANCE_PAGE_FAILED", message) }
            try await Task.sleep(for: .milliseconds(150))
        }
        let state = try await view.evaluateJavaScript("JSON.stringify({url:location.href,ready:document.readyState,sdk:!!window.localApp,task:window.__task,restored:window.__restored})")
        throw LocalAppServiceFailure("ACCEPTANCE_TIMEOUT", "\(condition): \(state ?? "unavailable")")
    }
    private func clean(_ library: UnifiedLocalApps, _ pairs: [(LocalAppPublishResult, URL)]) async {
        for (app, source) in pairs {
            try? await library.stop(app.record.appId)
            try? await library.browser.clearData(app.record.appId)
            try? await library.action("remove", id: app.record.appId)
            try? FileManager.default.removeItem(at: source)
        }
    }

    func testCancelRevokeAndStaleCloseOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Requires physical iPhone and configured provider")
        #else
        let client = LocalAppServiceClient()
        let owned = UnifiedLocalApps(client: client)
        let app = try await publish(client)
        do {
            try await configure(client, app.0.record.appId)
            let first = try await owned.launch(app.0.record.appId)
            let input: [String: Any] = ["requestId": "ios-cancel-" + UUID().uuidString, "prompt": "Count from 1 to 20000 on separate lines.", "tools": [] as [String]]
            let admitted = try await page(client, first, "agent.run", input)
            let runID = try XCTUnwrap(admitted["runId"] as? String)
            _ = try await page(client, first, "agent.cancel", ["runId": runID])
            let cancelled = try await page(client, first, "agent.get", ["runId": runID])
            XCTAssertEqual(cancelled["state"] as? String, "cancelled")
            try await owned.action("restart", id: app.0.record.appId)
            let replacement = try await owned.launch(app.0.record.appId)
            XCTAssertNotEqual(first.run.runId, replacement.run.runId)
            var nextInput = input; nextInput["requestId"] = "ios-next-" + UUID().uuidString
            let next = try await page(client, replacement, "agent.run", nextInput)
            let nextID = try XCTUnwrap(next["runId"] as? String)
            try await client.agentClosed(appID: first.run.appId, invocationID: first.run.runId)
            let current = try await page(client, replacement, "agent.get", ["runId": nextID])
            XCTAssertNotEqual(current["state"] as? String, "cancelled")
            _ = try await client.agentHost(json(["op": "configure", "appId": app.0.record.appId, "settings": NSNull()]))
            let revoked = try await page(client, replacement, "agent.get", ["runId": nextID])
            XCTAssertEqual(revoked["state"] as? String, "cancelled")
            nextInput["requestId"] = "ios-revoked-" + UUID().uuidString
            do { _ = try await page(client, replacement, "agent.run", nextInput); XCTFail("Revoked app restarted a task") }
            catch is LocalAppServiceFailure { }
            print("IOS_AGENT_CANCEL PASS actual-native cancel reap restart stale-close revoke no-grant-reuse")
        } catch { await clean(owned, [app]); throw error }
        await clean(owned, [app])
        #endif
    }

    func testRealImageSchemaUsageRetryRefreshAndAppIsolationOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Requires physical iPhone with a configured real vision model")
        #else
        let client = LocalAppServiceClient()
        // One service instance owns the page and trusted host operations.
        let owned = UnifiedLocalApps(client: client)
        var created: [(LocalAppPublishResult, URL)] = []
        var window: UIWindow?
        let scene = try XCTUnwrap(UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first { $0.activationState == .foregroundActive })
        let previous = scene.windows.first { $0.isKeyWindow }
        do {
            let app = try await publish(client); created.append(app)
            let other = try await publish(client); created.append(other)
            try await configure(client, app.0.record.appId)
            let web = try await owned.browser.open(app.0.record.appId) { _ in }
            let testWindow = UIWindow(windowScene: scene)
            let controller = UIViewController(); controller.view.addSubview(web)
            web.frame = scene.coordinateSpace.bounds; web.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            testWindow.rootViewController = controller; testWindow.makeKeyAndVisible(); window = testWindow
            try await wait(web, "document.readyState==='complete' && !!window.localApp")
            let bytes = try Data(contentsOf: app.1.appendingPathComponent("public-apple.jpg"))
            let input: [String: Any] = ["requestId": "ios-photo-" + UUID().uuidString,
                "prompt": "Identify the food in this public photograph. Return its English name in food and one visible detail in detail, using only the result tool.",
                "images": [["media_type": "image/jpeg", "base64": bytes.base64EncodedString()]], "tools": [] as [String],
                "outputSchema": ["type": "object", "properties": ["food": ["type": "string"], "detail": ["type": "string"]], "required": ["food", "detail"], "additionalProperties": false]]
            _ = try await web.evaluateJavaScript("window.__input=\(try json(input));localApp.agent.run(__input).then(v=>{window.__runId=v.runId;localApp.agent.subscribe(v.runId,e=>{window.__task=e.run;if(['failed','cancelled','interrupted'].includes(e.run.state))window.__agentFailure=JSON.stringify(e.run.error);if(e.run.state==='succeeded'){document.getElementById('result').textContent=JSON.stringify(e.run.result.output);localStorage.setItem('agent-result',JSON.stringify(e.run.result.output));}});}).catch(e=>window.__agentFailure=String(e));void 0")
            try await wait(web, "window.__task?.state==='succeeded'", seconds: 180)
            let valid = try await web.evaluateJavaScript("__task.result.output.food.toLowerCase().includes('apple') && __task.usage.outputTokens>0 && __task.usage.turns>0 && typeof __task.usage.costEstimated==='boolean'") as? Bool
            XCTAssertEqual(valid, true)
            let runValue = try await web.evaluateJavaScript("window.__runId") as? String
            let runID = try XCTUnwrap(runValue)
            _ = try await web.evaluateJavaScript("localApp.agent.run(__input).then(v=>window.__retryId=v.runId).catch(e=>window.__agentFailure=String(e));void 0")
            try await wait(web, "window.__retryId===window.__runId")
            _ = try await web.evaluateJavaScript("localApp.agent.run({...__input,prompt:'conflict'}).then(()=>window.__agentFailure='conflict admitted').catch(()=>window.__conflictDenied=true);void 0")
            try await wait(web, "window.__conflictDenied===true")
            let otherLaunch = try await owned.launch(other.0.record.appId)
            do { _ = try await page(client, otherLaunch, "agent.get", ["runId": runID]); XCTFail("Another app read this task") }
            catch is LocalAppServiceFailure { }
            _ = try await web.evaluateJavaScript("window.__oldDocumentBeforeReload=true")
            web.reload()
            try await wait(web, "!window.__oldDocumentBeforeReload && document.readyState==='complete' && !!window.localApp")
            let quotedID = try json(["id": runID])
            _ = try await web.evaluateJavaScript("localApp.agent.get((\(quotedID)).id).then(v=>window.__restored=v).catch(e=>window.__agentFailure=String(e));void 0")
            try await wait(web, "window.__restored?.state==='succeeded'")
            let retained = try await web.evaluateJavaScript("JSON.stringify(__restored.result.output)===localStorage.getItem('agent-result')") as? Bool
            XCTAssertEqual(retained, true)
            print("IOS_AGENT_IMAGE PASS actual-model schema positive-usage retry conflict app-isolation WK-reload run=\(runID)")
        } catch {
            await clean(owned, created); window?.isHidden = true; previous?.makeKeyAndVisible(); throw error
        }
        await clean(owned, created); window?.isHidden = true; previous?.makeKeyAndVisible()
        #endif
    }
}
