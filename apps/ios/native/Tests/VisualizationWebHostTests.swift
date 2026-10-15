import Combine
import WebKit
import XCTest
@testable import LingxiCode

@MainActor
final class VisualizationWebHostTests: XCTestCase {
    func testSuspendedWidgetCanLoadAgain() async {
        let host = ShellHost()
        VisualizationWebHost.shared.attach(host)
        defer { VisualizationWebHost.shared.attach(nil) }
        let controller = VisualizationWidgetController(sessionId: "session", id: "chart", revision: 1, dark: false)
        defer { controller.suspend() }

        for _ in 0..<2 {
            let ready = expectation(description: "shell handshake completes")
            let observation = controller.$phase.filter { $0 == .ready }.prefix(1).sink { _ in ready.fulfill() }
            controller.makeWebView { XCTAssertNotNil($0) }
            await fulfillment(of: [ready], timeout: 10)
            observation.cancel()
            XCTAssertEqual(controller.phase, .ready)
            controller.suspend()
            XCTAssertNil(controller.webView)
        }
    }

    func testMountFinishingAfterSuspensionIsReleased() async {
        let started = expectation(description: "mount started")
        let released = expectation(description: "late mount released")
        let gate = DispatchSemaphore(value: 0)
        let host = ShellHost(onMount: {
            started.fulfill()
            _ = gate.wait(timeout: .now() + 10)
        }, onUnmount: { token in
            XCTAssertEqual(token, "test-mount")
            released.fulfill()
        })
        VisualizationWebHost.shared.attach(host)
        defer { VisualizationWebHost.shared.attach(nil) }
        let controller = VisualizationWidgetController(sessionId: "session", id: "chart", revision: 1, dark: false)
        controller.makeWebView { XCTAssertNotNil($0) }
        await fulfillment(of: [started], timeout: 10)
        controller.suspend()
        gate.signal()
        await fulfillment(of: [released], timeout: 10)
        XCTAssertNil(controller.webView)
        XCTAssertEqual(controller.phase, .loading)
    }
}

/// Only the native shell handshake is needed to exercise controller ownership.
private final class ShellHost: VisualizationHost, @unchecked Sendable {
    private let onMount: () -> Void
    private let onUnmount: (String) -> Void

    init(onMount: @escaping () -> Void = {}, onUnmount: @escaping (String) -> Void = { _ in }) {
        self.onMount = onMount
        self.onUnmount = onUnmount
        super.init(noHandle: .init())
    }

    required init(unsafeFromHandle handle: UInt64) { fatalError("Test host has no FFI handle") }

    override func mount(sessionId: String, id: String, revision: UInt32, themeDto: VisualizationThemeDto,
                        locale: String, expanded: Bool) -> VisualizationMountDto? {
        onMount()
        return VisualizationMountDto(token: "test-mount", generation: 1,
                                     docUrl: "lingxi-viz://visualization/doc/test", title: "Chart")
    }

    override func unmount(token: String) { onUnmount(token) }

    override func serve(path: String) -> VisualizationResponseDto {
        let html = """
        <!doctype html><script>
        const bridge = window.webkit.messageHandlers.lingxiVisualization;
        window.__lingxiVisualizationDeliver = raw => {
          const message = JSON.parse(raw);
          if (message.type === 'mount') {
            bridge.postMessage(JSON.stringify({type:'ready', generation:message.generation}));
          }
        };
        bridge.postMessage(JSON.stringify({type:'shell.ready'}));
        </script>
        """
        return VisualizationResponseDto(status: 200, mimeType: "text/html",
                                        headers: [.init(name: "Content-Type", value: "text/html; charset=utf-8")],
                                        body: Data(html.utf8))
    }
}
