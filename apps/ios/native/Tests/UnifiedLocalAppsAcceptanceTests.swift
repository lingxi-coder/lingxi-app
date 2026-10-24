import Foundation
import XCTest
import WebKit
@testable import LingxiCode

/// Requires a signed product on a physical iPhone. Simulator success is not Linux proof.
@MainActor
final class UnifiedLocalAppsAcceptanceTests: XCTestCase {
    func testWorkspaceAndExternalOverlayNeverModifyHiddenHomeOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Physical iPhone iSH filesystem routing acceptance")
        #else
        LXISHRuntimeResourceBootstrap.prepare()
        let metadata = LXISHRuntimeBundleMetadata.current()
        let storage = MobileUserStorage.directories()
        let existingTarget = (try? FileManager.default.destinationOfSymbolicLink(atPath: storage.projects)) ?? "<not a symlink>"
        print("IOS_LOCAL_APP_WORKSPACE_PREPARATION projects=\(storage.projects) linkTarget=\(existingTarget)")
        let projects = try MobileUserStorage.prepareProjectsDirectory()
        let marker = "overlay-acceptance-" + UUID().uuidString
        let projectSentinel = projects.appendingPathComponent(marker)
        try Data("keep-workspace-source".utf8).write(to: projectSentinel)
        let guest = "/home/lingxi/" + marker
        let hidden = try UnifiedLocalApps.hostSourceDirectory(guest)
        let external = URL(fileURLWithPath: ConversationSourceFactory.appSandboxRoot())
            .appendingPathComponent(marker, isDirectory: true)
        for directory in [hidden, external, external.appendingPathComponent("ro"), external.appendingPathComponent("writable")] {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        let original = Data("HOME-MUST-STAY-BYTE-FOR-BYTE".utf8)
        try original.write(to: hidden.appendingPathComponent("same.txt"))
        try Data("external-value".utf8).write(to: external.appendingPathComponent("same.txt"))
        try Data("cold-external-only".utf8).write(to: external.appendingPathComponent("cold-only.txt"))
        try Data("read-only".utf8).write(to: external.appendingPathComponent("ro/locked.txt"))
        let runtime = try createIosMobileLinuxRuntime(config: IosMobileLinuxConfigFfi(
            mode: .mobileLinux, managedRoot: LXISHDefaultWorkspace.managedRootPath(),
            workspaceHostPath: projects.path, stableWorkspaceId: LXISHDefaultWorkspace.stableID(),
            abi: "arm64", rootfsVersion: metadata.rootfsVersion, archiveSha256: metadata.archiveSha256,
            authorizationFile: LXISHRuntimeBundleResources.authorizationManifestURL()?.path,
            appSandboxRoot: ConversationSourceFactory.appSandboxRoot()))
        func run(_ script: String, cwd: String = "/", mounts: [MobileLinuxMountSpecFfi] = []) async throws {
            let result = try await runtime.runCommand(request: MobileLinuxCommandRequestFfi(
                command: "/usr/bin/python3", args: ["-c", script], cwd: cwd, env: [:], stdin: nil,
                timeoutMs: 30000, network: .disabled, mounts: mounts))
            XCTAssertEqual(result.exitCode, 0, result.stderr)
            XCTAssertFalse(result.timedOut)
        }
        do {
            _ = try await runtime.boot()
            let mounts = [
                MobileLinuxMountSpecFfi(hostPath: external.path, guestPath: guest, readOnly: false, purpose: .external),
                MobileLinuxMountSpecFfi(hostPath: external.appendingPathComponent("ro").path, guestPath: guest + "/ro", readOnly: true, purpose: .external),
                MobileLinuxMountSpecFfi(hostPath: external.appendingPathComponent("writable").path, guestPath: guest + "/ro/writable", readOnly: false, purpose: .external)
            ]
            try await run("""
            import os, errno, stat
            assert os.stat('cold-only.txt').st_size == 18
            assert os.stat('same.txt').st_size == 14
            os.truncate('same.txt', 3)
            os.link('same.txt', 'hard.txt')
            assert open('hard.txt').read() == 'ext'
            os.symlink('same.txt', 'soft.txt')
            assert os.readlink('soft.txt') == 'same.txt'
            assert open('soft.txt').read() == 'ext'
            os.mkfifo('pipe')
            assert stat.S_ISFIFO(os.stat('pipe').st_mode)
            os.mkdir('new-directory'); os.rename('new-directory', 'renamed-directory'); os.rmdir('renamed-directory')
            os.unlink('hard.txt')
            try:
                os.truncate('ro/locked.txt', 0)
                raise AssertionError('read-only overlay allowed truncate')
            except OSError as error:
                assert error.errno == errno.EROFS, str(error)
            try:
                os.link('ro/locked.txt', 'read-only-alias.txt')
                raise AssertionError('read-only overlay allowed a writable hardlink alias')
            except OSError as error:
                assert error.errno == errno.EROFS, str(error)
            open('ro/writable/result.txt', 'w').write('nearest-overlay-writable')
            """, cwd: guest, mounts: mounts)
            XCTAssertEqual(try Data(contentsOf: hidden.appendingPathComponent("same.txt")), original)
            XCTAssertEqual(try String(contentsOf: external.appendingPathComponent("same.txt"), encoding: .utf8), "ext")
            XCTAssertEqual(try String(contentsOf: external.appendingPathComponent("writable/result.txt"), encoding: .utf8), "nearest-overlay-writable")
            XCTAssertEqual(try String(contentsOf: external.appendingPathComponent("ro/locked.txt"), encoding: .utf8), "read-only")
            XCTAssertFalse(FileManager.default.fileExists(atPath: hidden.appendingPathComponent("soft.txt").path))
            XCTAssertFalse(FileManager.default.fileExists(atPath: hidden.appendingPathComponent("pipe").path))
            _ = try await runtime.configureMounts(mounts: [MobileLinuxMountSpecFfi(
                hostPath: projects.path, guestPath: "/home/lingxi/lingxi/projects", readOnly: true, purpose: .workspace)])
            try await run("""
            import os, errno
            try:
                os.truncate('/home/lingxi/lingxi/projects/\(marker)', 0)
                raise AssertionError('default workspace ignored read-only request')
            except OSError as error:
                assert error.errno == errno.EROFS, str(error)
            """)

            // Adding and removing an ancestor rebuilds the child's physical /
            // table-only representation, without stranding its mount owner.
            let parentGuest = "/var/lingxi/" + marker
            let child = MobileLinuxMountSpecFfi(hostPath: external.path, guestPath: parentGuest + "/job", readOnly: false, purpose: .external)
            _ = try await runtime.configureMounts(mounts: [child])
            try await run("assert open('same.txt').read() == 'ext'", cwd: child.guestPath)
            let parent = MobileLinuxMountSpecFfi(hostPath: external.appendingPathComponent("writable").path, guestPath: parentGuest, readOnly: false, purpose: .external)
            _ = try await runtime.configureMounts(mounts: [parent, child])
            try await run("assert open('same.txt').read() == 'ext'", cwd: child.guestPath)
            _ = try await runtime.configureMounts(mounts: [child])
            try await run("assert open('same.txt').read() == 'ext'", cwd: child.guestPath)
            _ = try await runtime.configureMounts(mounts: [])
            XCTAssertEqual(try Data(contentsOf: projectSentinel), Data("keep-workspace-source".utf8))
            XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: projects.path)[.type] as? FileAttributeType, .typeDirectory)
            print("IOS_LOCAL_APP_OVERLAY PASS cold-stat truncate hardlink fake-symlink readlink fifo readonly nearest-RW ancestor-transition HOME-and-workspace-preserved")
        } catch {
            _ = try? await runtime.configureMounts(mounts: [])
            throw error
        }
        try FileManager.default.removeItem(at: hidden)
        try FileManager.default.removeItem(at: external)
        try FileManager.default.removeItem(at: projectSentinel)
        #endif
    }

    func testStaticBrowserAndSourceArchiveOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Physical iPhone browser and source roundtrip acceptance")
        #else
        let library = UnifiedLocalApps()
        let fixtures = try XCTUnwrap(Bundle(for: Self.self).resourceURL).appendingPathComponent("UnifiedAppFixtures/static")
        let published: LocalAppPublishResult = try await library.client.request(["op": .string("publish"), "expectedRevision": .number(0), "sourcePath": .string(fixtures.path)])
        var created = [published.record.appId]
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("source-roundtrip-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        do {
            await library.refresh()
            let archive = try await library.sourceArchive(published.record.appId, revision: published.release.revision)
            XCTAssertEqual(Set(archive.files), ["lingxi-app.json", "dist/index.html", "dist/client.js"])
            let zip = folder.appendingPathComponent("source.zip"); try archive.bytes.write(to: zip)
            let imported = try await library.importSource(zip)
            created.append(imported.record.appId)
            XCTAssertNotEqual(imported.record.appId, published.record.appId)
            XCTAssertNil(imported.record.marketOrigin)
            XCTAssertTrue(imported.record.executionGrants.isEmpty)
            await library.refresh()
            func awaitPage(_ view: WKWebView) async throws {
                for _ in 0..<150 {
                    if (try? await view.evaluateJavaScript("document.readyState === 'complete' && !!window.localApp")) as? Bool == true { return }
                    try await Task.sleep(for: .milliseconds(100))
                }
                throw LocalAppServiceFailure("BROWSER_TIMEOUT", "The actual WKWebView did not load the application SDK.")
            }
            let page = try await library.browser.open(published.record.appId) { _ in }
            try await awaitPage(page)
            let origin = try await page.evaluateJavaScript("location.origin") as? String
            let info = try await page.callAsyncJavaScript("return await localApp.info()", arguments: [:], in: nil, contentWorld: .page) as? [String: Any]
            XCTAssertEqual(info?["runId"] as? String, library.runs[published.record.appId]?.runId)
            XCTAssertTrue((info?["dataDirectory"] as? String)?.hasPrefix("/home/lingxi/") == true)
            _ = try await page.evaluateJavaScript("localStorage.setItem('acceptance-note', 'keep-on-restart')")
            let denied = try await page.callAsyncJavaScript("try { await localApp.notify('must-not-post'); return false; } catch (_) { return true; }", arguments: [:], in: nil, contentWorld: .page) as? Bool
            XCTAssertEqual(denied, true)
            try await library.stop(published.record.appId)
            let reopened = try await library.browser.open(published.record.appId) { _ in }
            try await awaitPage(reopened)
            let reopenedOrigin = try await reopened.evaluateJavaScript("location.origin") as? String
            let retainedNote = try await reopened.evaluateJavaScript("localStorage.getItem('acceptance-note')") as? String
            XCTAssertEqual(reopenedOrigin, origin)
            XCTAssertEqual(retainedNote, "keep-on-restart")
            let other = try await library.browser.open(imported.record.appId) { _ in }
            try await awaitPage(other)
            let otherNote = try await other.evaluateJavaScript("localStorage.getItem('acceptance-note')") as? String
            XCTAssertNil(otherNote)
            try await library.stop(published.record.appId)
            try await library.browser.clearData(published.record.appId)
            print("IOS_LOCAL_APP_BROWSER_SOURCE PASS actual WKWebView stable-origin storage-isolation source-export-import permission-denial data-clear")
        } catch {
            for id in created { try? await library.stop(id); try? await library.action("remove", id: id) }
            try? FileManager.default.removeItem(at: folder)
            throw error
        }
        for id in created { try await library.stop(id); try await library.browser.clearData(id); try await library.action("remove", id: id) }
        try FileManager.default.removeItem(at: folder)
        #endif
    }

    func testStaticPythonNodeAndOwnedLifecycleOnDevice() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("A physical iPhone is required for the Local App Linux chain")
        #else
        let library = UnifiedLocalApps()
        let client = library.client
        let fixtures = try XCTUnwrap(Bundle(for: Self.self).resourceURL).appendingPathComponent("UnifiedAppFixtures")
        var created: [String] = [], sources: [URL] = []
        var phase = "publish-static"
        func checkpoint(_ value: String) {
            phase = value
            print("IOS_LOCAL_APP_CHECKPOINT \(value) cancelled=\(Task.isCancelled)")
        }
        let session = URLSession(configuration: .ephemeral)
        func publish(_ name: String) async throws -> LocalAppPublishResult {
            let guest = "/home/lingxi/.lingxi/.apps/acceptance-\(UUID().uuidString)"
            let source = try UnifiedLocalApps.hostSourceDirectory(guest)
            try FileManager.default.createDirectory(at: source.deletingLastPathComponent(), withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: fixtures.appendingPathComponent(name), to: source)
            sources.append(source)
            let result: LocalAppPublishResult = try await client.request([
                "op": .string("publish"), "expectedRevision": .number(0), "sourcePath": .string(source.path),
                "authoring": try .encode(LocalAppAuthoring(sourceDirectory: guest, sourceSessionId: "acceptance-source"))
            ])
            created.append(result.record.appId)
            return result
        }
        func start(_ id: String, operation: String = "start") async throws -> LocalAppLaunch {
            try await client.request(["op": .string(operation), "appId": .string(id)])
        }
        func url(_ launch: LocalAppLaunch, _ path: String) throws -> URL {
            try XCTUnwrap(URL(string: launch.origin + "/__run/\(launch.run.runId)/__service/api" + path))
        }
        func request(_ launch: LocalAppLaunch, _ path: String, body: Data? = nil) throws -> URLRequest {
            var value = URLRequest(url: try url(launch, path)); value.timeoutInterval = 20
            value.setValue(launch.origin, forHTTPHeaderField: "Origin")
            if let body { value.httpMethod = "POST"; value.httpBody = body; value.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type") }
            return value
        }
        func http(_ launch: LocalAppLaunch, _ path: String, body: Data? = nil) async throws -> (Data, Int) {
            let (bytes, response) = try await session.data(for: request(launch, path, body: body))
            return (bytes, try XCTUnwrap(response as? HTTPURLResponse).statusCode)
        }
        do {
            let document = try await publish("static")
            let staticLaunch = try await start(document.record.appId)
            let (html, response) = try await session.data(from: staticLaunch.url)
            XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
            XCTAssertTrue(String(decoding: html, as: UTF8.self).contains("html"))
            try FileManager.default.removeItem(at: sources[0])
            let resolution: LocalAppAuthoringResolution = try await client.request([
                "op": .string("prepareAuthoring"), "appId": .string(document.record.appId), "sessionDirectories": .object([:])
            ])
            XCTAssertEqual(resolution.action, "sourceMissing")
            let (_, stillAvailable) = try await session.data(from: staticLaunch.url)
            XCTAssertEqual((stillAvailable as? HTTPURLResponse)?.statusCode, 200)
            print("IOS_LOCAL_APP_STATIC PASS source-independent immutable release")

            checkpoint("publish-services")
            let python = try await publish("python"), node = try await publish("node")
            for app in [python, node] {
                do { _ = try await start(app.record.appId); XCTFail("Execution requires exact release consent") }
                catch is LocalAppServiceFailure { }
                try await library.grant(app.record.appId, digest: app.release.digest)
            }
            checkpoint("start-python")
            let py = try await start(python.record.appId)
            checkpoint("start-node")
            let js = try await start(node.record.appId)
            let bytes = Data([0, 1, 255, 42])
            for (launch, expected) in [(py, "PYTHON-READY"), (js, "NODE-READY")] {
                checkpoint("http-" + expected)
                let (health, code) = try await http(launch, "/health")
                XCTAssertEqual(code, 200); XCTAssertEqual(String(decoding: health, as: UTF8.self), expected)
                let (echo, echoedCode) = try await http(launch, "/echo", body: bytes)
                XCTAssertEqual(echoedCode, 200); XCTAssertEqual(echo, bytes)
                checkpoint("sse-" + expected)
                let streamSession = URLSession(configuration: .ephemeral)
                let (stream, _) = try await streamSession.bytes(for: request(launch, "/events"))
                var lines = stream.lines.makeAsyncIterator()
                let first = try await lines.next()
                XCTAssertEqual(first, "data: 0")
                streamSession.invalidateAndCancel()
            }
            checkpoint("persistent-data")
            let (saved, _) = try await http(py, "/save", body: bytes)
            XCTAssertEqual(saved, bytes)
            let layout = try await library.layout(python.record.appId)
            let dataFile = URL(fileURLWithPath: layout.dataPath).appendingPathComponent("acceptance.bin")
            XCTAssertEqual(try Data(contentsOf: dataFile), bytes)
            var socketRequest = try request(js, "/socket")
            var parts = try XCTUnwrap(URLComponents(url: socketRequest.url!, resolvingAgainstBaseURL: false)); parts.scheme = "ws"
            socketRequest.url = parts.url
            let socket = session.webSocketTask(with: socketRequest); socket.resume()
            checkpoint("websocket")
            try await socket.send(.data(bytes))
            let echo = try await socket.receive()
            if case .data(let value) = echo { XCTAssertEqual(value, bytes) } else { XCTFail("Binary WebSocket message changed type") }
            let closed = expectation(description: "old WebSocket closes when its run ends")
            let receive = Task { do { _ = try await socket.receive() } catch { closed.fulfill() } }
            try await library.stop(python.record.appId)
            checkpoint("owned-stop-and-restart")
            let (nodeAlive, _) = try await http(js, "/health")
            XCTAssertEqual(String(decoding: nodeAlive, as: UTF8.self), "NODE-READY")
            let restarted = try await start(node.record.appId, operation: "restart")
            XCTAssertEqual(js.origin, restarted.origin); XCTAssertNotEqual(js.run.runId, restarted.run.runId)
            let (_, staleCode) = try await http(js, "/health"); XCTAssertEqual(staleCode, 410)
            await fulfillment(of: [closed], timeout: 10)
            receive.cancel(); socket.cancel(with: .normalClosure, reason: nil)
            checkpoint("repair-and-revoke")
            let repaired = try await start(python.record.appId, operation: "repair")
            XCTAssertEqual(py.origin, repaired.origin); XCTAssertEqual(try Data(contentsOf: dataFile), bytes)
            try await library.action("revoke", id: node.record.appId)
            do { _ = try await start(node.record.appId); XCTFail("Revoked execution must not start") }
            catch is LocalAppServiceFailure { }
            print("IOS_LOCAL_APP_SERVICES PASS real Python+Node POST binary SSE-cancel WebSocket restart data owned-stop revoke")
        } catch {
            print("IOS_LOCAL_APP_FAILURE phase=\(phase) type=\(String(reflecting: type(of: error))) error=\(error) cancelled=\(Task.isCancelled)")
            for id in created {
                if let history: [UnifiedAppRun] = try? await client.request(["op": .string("runs"), "appId": .string(id)]) {
                    for run in history {
                        let output = (try? await library.logs(id, runID: run.runId)) ?? "logs unavailable"
                        print("IOS_LOCAL_APP_FAILURE_RUN app=\(id) run=\(run.runId) phase=\(run.phase) error=\(run.error ?? "none") logs=\(output.suffix(4000))")
                    }
                }
            }
            for id in created { try? await library.stop(id) }
            session.invalidateAndCancel()
            throw error
        }
        for id in created { try await library.stop(id) }
        for source in sources where FileManager.default.fileExists(atPath: source.path) { try FileManager.default.removeItem(at: source) }
        session.invalidateAndCancel()
        #endif
    }
}
