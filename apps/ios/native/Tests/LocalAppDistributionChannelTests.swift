import Foundation
import XCTest
@testable import LingxiCode

/// Run against both FullDebug and StoreDebug; a signed build alone does not
/// establish that a restricted channel refuses downloaded Linux code.
@MainActor
final class LocalAppDistributionChannelTests: XCTestCase {
    func testInstalledChannelAdvertisesOnlyItsExecutionEnvironments() {
        let full = Bundle.main.bundleIdentifier == "com.lingxi.code.full"
        let targets = UnifiedAppMarket.targets
        XCTAssertEqual(Set(targets.map(\.environment)), full ? ["web", "mobile-linux"] : ["web"])
        XCTAssertTrue(targets.allSatisfy { $0.platform == "ios" })
        XCTAssertTrue(targets.allSatisfy { $0.channels == [full ? "full" : "store"] })
        for target in targets where !full {
            XCTAssertFalse(target.capabilities?.contains("process") ?? false)
            XCTAssertFalse(target.capabilities?.contains("background") ?? false)
        }
    }

    func testStoreRejectsGrantedLinuxReleaseBeforeRuntimePreparation() async throws {
        guard Bundle.main.bundleIdentifier != "com.lingxi.code.full" else {
            throw XCTSkip("Requires the actual StoreDebug product, not a simulated channel flag")
        }
        let source = try UnifiedLocalApps.hostSourceDirectory("/home/lingxi/.lingxi/.apps/channel-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: source) }
        let manifest = #"{"schema":2,"name":"Channel refusal fixture","entry":{"type":"server","process":"api"},"targets":[{"platform":"ios","environment":"mobile-linux","channels":["full"],"capabilities":["process"]}],"capabilities":["process"],"processes":[{"name":"api","command":"python3 server.py","ready":{"path":"/","timeoutMs":1000}}]}"#
        try Data(manifest.utf8).write(to: source.appendingPathComponent("lingxi-app.json"))
        try Data("raise RuntimeError('restricted channel must never execute this')".utf8).write(to: source.appendingPathComponent("server.py"))
        let client = LocalAppServiceClient()
        let published: LocalAppPublishResult = try await client.request([
            "op": .string("publish"), "sourcePath": .string(source.path), "expectedRevision": .number(0)
        ])
        let id = published.record.appId
        do {
            let _: LocalAppJSON = try await client.request(["op": .string("grant"), "appId": .string(id), "digest": .string(published.release.digest)])
            for operation in ["start", "resume", "restart", "repair"] {
                do {
                    let _: LocalAppJSON = try await client.request(["op": .string(operation), "appId": .string(id)])
                    XCTFail("Store channel allowed \(operation)")
                } catch let failure as LocalAppServiceFailure {
                    XCTAssertEqual(failure.code, "UNSUPPORTED_CHANNEL", "\(operation): \(failure)")
                }
            }
            let runs: [UnifiedAppRun] = try await client.request(["op": .string("runs"), "appId": .string(id)])
            XCTAssertTrue(runs.isEmpty, "Rejected channel must not create a preparation or run")
        } catch {
            let _: LocalAppJSON? = try? await client.request(["op": .string("remove"), "appId": .string(id)])
            throw error
        }
        let _: LocalAppJSON = try await client.request(["op": .string("remove"), "appId": .string(id)])
        print("IOS_LOCAL_APP_CHANNEL PASS actual Store product rejects granted Linux start/resume/restart/repair")
    }
}
