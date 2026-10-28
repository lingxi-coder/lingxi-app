import SwiftUI
import WebKit

/// Transcript-only offline renderer. This web view exposes no application tools.
struct MermaidDiagram: View {
    @Environment(\.theme) private var theme
    let source: String
    @State private var height: CGFloat = 300
    @State private var failed = false

    var body: some View {
        Group {
            if let document = Self.document, !failed {
                MermaidWebView(source: source, dark: theme.isDark, document: document, height: $height, failed: $failed)
                    .frame(height: height)
            } else {
                ScrollView(.horizontal) {
                    Text(source).font(.system(size: 13, design: .monospaced)).textSelection(.enabled).padding(12)
                }
            }
        }
        .onChange(of: source) { _, _ in failed = false }
    }

    private static let document: String? = Bundle.main.url(forResource: "mermaid", withExtension: "html")
        .flatMap { try? String(contentsOf: $0, encoding: .utf8) }
}

private struct MermaidWebView: UIViewRepresentable {
    let source: String
    let dark: Bool
    let document: String
    @Binding var height: CGFloat
    @Binding var failed: Bool

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.userContentController.add(context.coordinator, name: "mermaid")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        view.navigationDelegate = context.coordinator
        context.coordinator.view = view
        view.loadHTMLString(document, baseURL: nil)
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.onHeight = { value in
            if height != value { height = value }
        }
        context.coordinator.onFailure = { failed = true }
        context.coordinator.update(source: source, dark: dark)
    }

    static func dismantleUIView(_ view: WKWebView, coordinator: Coordinator) {
        view.stopLoading()
        view.navigationDelegate = nil
        view.configuration.userContentController.removeScriptMessageHandler(forName: "mermaid")
        coordinator.view = nil
        coordinator.onHeight = nil
        coordinator.onFailure = nil
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        weak var view: WKWebView?
        var onHeight: ((CGFloat) -> Void)?
        var onFailure: (() -> Void)?
        private var ready = false
        private var payload: String?
        private var delivered: String?

        func update(source: String, dark: Bool) {
            guard let data = try? JSONSerialization.data(withJSONObject: ["source": source, "dark": dark]),
                  let json = String(data: data, encoding: .utf8) else { return }
            payload = json
            deliver()
        }

        private func deliver() {
            guard ready, let payload, payload != delivered else { return }
            delivered = payload
            view?.evaluateJavaScript("window.renderMermaid(\(payload))")
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.webView === view, message.frameInfo.isMainFrame,
                  let data = message.body as? [String: Any], data["channel"] as? String == "lingxi-mermaid" else { return }
            if data["type"] as? String == "ready" {
                ready = true
                deliver()
            }
            if data["type"] as? String == "size", let value = data["height"] as? Double, value.isFinite {
                onHeight?(min(640, max(180, CGFloat(value))))
            }
        }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            decisionHandler(navigationAction.request.url?.absoluteString == "about:blank" ? .allow : .cancel)
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { onFailure?() }
    }
}
