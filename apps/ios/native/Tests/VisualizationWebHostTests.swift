import Combine
import WebKit
import XCTest
@testable import LingxiCode

@MainActor
final class VisualizationWebHostTests: XCTestCase {
    func testConcurrentWebViewRequestsShareOneDocument() async {
        VisualizationWebHost.shared.attach(ShellHost())
        defer { VisualizationWebHost.shared.attach(nil) }
        let controller = VisualizationWidgetController(sessionId: "session", id: "chart", revision: 1, dark: false)
        defer { controller.suspend() }
        let created = expectation(description: "both presenters receive a web view")
        created.expectedFulfillmentCount = 2
        let ready = expectation(description: "shared document ready")
        let observation = controller.$phase.filter { $0 == .ready }.prefix(1).sink { _ in ready.fulfill() }
        var views: [WKWebView] = []
        for _ in 0..<2 {
            controller.makeWebView { view in
                if let view { views.append(view) }
                created.fulfill()
            }
        }
        await fulfillment(of: [created, ready], timeout: 10)
        observation.cancel()
        XCTAssertEqual(views.count, 2)
        if views.count == 2 { XCTAssertTrue(views[0] === views[1]) }
    }

    func testExpansionMovesTheSameWebViewWithoutRemounting() async {
        let host = ShellHost()
        VisualizationWebHost.shared.attach(host)
        defer { VisualizationWebHost.shared.attach(nil) }
        let controller = VisualizationWidgetController(sessionId: "session", id: "chart", revision: 1, dark: false)
        defer { controller.suspend() }
        let inline = UIView()
        let expanded = UIView()
        let ready = expectation(description: "inline widget ready")
        let observation = controller.$phase.filter { $0 == .ready }.prefix(1).sink { _ in ready.fulfill() }
        controller.attachWebView(to: inline, expanded: false)
        await fulfillment(of: [ready], timeout: 10)
        observation.cancel()
        let original = controller.webView
        XCTAssertNotNil(original)
        XCTAssertTrue(original?.superview === inline)

        controller.setExpanded(true)
        controller.attachWebView(to: expanded, expanded: true)
        XCTAssertTrue(controller.isExpanded)
        XCTAssertTrue(controller.webView === original)
        XCTAssertTrue(original?.superview === expanded)
        XCTAssertEqual(controller.phase, .ready)
        let expandedState = try? await original?.evaluateJavaScript("window.expanded")
        XCTAssertEqual(expandedState as? Bool, true)

        controller.detachWebView(from: expanded, expanded: true)
        XCTAssertFalse(controller.isExpanded)
        XCTAssertTrue(controller.webView === original)
        XCTAssertTrue(original?.superview === inline)
        XCTAssertEqual(controller.phase, .ready)
        let inlineState = try? await original?.evaluateJavaScript("window.expanded")
        XCTAssertEqual(inlineState as? Bool, false)

        let mountCount = try? await original?.evaluateJavaScript("window.mountCount")
        XCTAssertEqual((mountCount as? NSNumber)?.intValue, 1)
        controller.detachWebView(from: inline, expanded: false)
        XCTAssertNil(controller.webView)
    }

    func testRetryAfterHostBecomesAvailableReusesTheInlineContainer() async {
        VisualizationWebHost.shared.attach(nil)
        defer { VisualizationWebHost.shared.attach(nil) }
        let controller = VisualizationWidgetController(sessionId: "session", id: "chart", revision: 1, dark: false)
        defer { controller.suspend() }
        let inline = UIView()
        controller.attachWebView(to: inline, expanded: false)
        XCTAssertEqual(controller.phase, .unavailable)
        XCTAssertNil(controller.webView)

        VisualizationWebHost.shared.attach(ShellHost())
        let ready = expectation(description: "retried widget ready")
        let observation = controller.$phase.filter { $0 == .ready }.prefix(1).sink { _ in ready.fulfill() }
        controller.retry()
        await fulfillment(of: [ready], timeout: 10)
        observation.cancel()
        XCTAssertTrue(controller.webView?.superview === inline)
        XCTAssertEqual(controller.phase, .ready)
    }

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
        window.mountCount = 0;
        window.__lingxiVisualizationDeliver = raw => {
          const message = JSON.parse(raw);
          if (message.type === 'mount') {
            window.mountCount += 1;
            window.expanded = message.expanded;
            bridge.postMessage(JSON.stringify({type:'ready', generation:message.generation}));
          }
          if (message.type === 'expanded') window.expanded = message.expanded;
        };
        bridge.postMessage(JSON.stringify({type:'shell.ready'}));
        </script>
        """
        return VisualizationResponseDto(status: 200, mimeType: "text/html",
                                        headers: [.init(name: "Content-Type", value: "text/html; charset=utf-8")],
                                        body: Data(html.utf8))
    }
}
