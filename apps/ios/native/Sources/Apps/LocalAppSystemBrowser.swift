import SafariServices
import SwiftUI

/// A transient presentation; never persist an application launch URL.
struct LocalAppBrowserTicket: Identifiable {
    let id = UUID()
    let url: URL
}

struct LocalAppSystemBrowser: UIViewControllerRepresentable {
    let url: URL
    func makeUIViewController(context: Context) -> SFSafariViewController { SFSafariViewController(url: url) }
    func updateUIViewController(_ controller: SFSafariViewController, context: Context) { }
}
