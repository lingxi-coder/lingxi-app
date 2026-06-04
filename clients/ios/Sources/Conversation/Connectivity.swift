import SwiftUI
import Network

// MARK: - Connectivity monitor (Network framework)
//
// A lightweight reachability observer backed by `NWPathMonitor`. It publishes a
// single `isOffline` flag the chat view binds to so it can surface an offline
// banner the moment the device loses connectivity (and clear it on recovery).
//
// The engine runs ON the device, but a turn still needs the network to reach the
// LLM API — so an offline state is worth telling the user about up front (and
// offering a retry) rather than letting a send fail opaquely with a transport
// error. This is purely additive: it observes the path, it never gates sends.

/// Observes network reachability via `NWPathMonitor` and publishes `isOffline`.
///
/// `NWPathMonitor` delivers updates on a background queue; we hop to the main
/// actor before mutating the `@Published` flag so SwiftUI observation stays on
/// the main thread. The monitor starts on `init` and is cancelled on `deinit`.
@MainActor
final class ConnectivityMonitor: ObservableObject {
    /// True when the current network path is NOT satisfied (no usable route).
    /// Seeded `false` (optimistic) until the first path update lands.
    @Published var isOffline = false

    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "com.lingxi.code.connectivity")

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let offline = path.status != .satisfied
            // The handler fires on `queue`; bounce to the main actor to mutate
            // published state and drive SwiftUI.
            Task { @MainActor [weak self] in
                self?.isOffline = offline
            }
        }
        monitor.start(queue: queue)
    }

    deinit {
        monitor.cancel()
    }
}

// MARK: - Offline banner

/// A persistent, non-dismissible (auto-clearing) banner shown while the device
/// is offline. It informs the user and offers an optional retry — useful when a
/// send is pending or the user wants to re-check reachability after toggling
/// Wi-Fi / airplane mode. The banner disappears on its own once connectivity is
/// restored (the monitor flips `isOffline` back to `false`).
struct OfflineBanner: View {
    @Environment(\.theme) private var t
    /// Optional retry affordance (e.g. re-warm the engine / re-send the draft).
    /// When `nil`, only the informational row is shown.
    var onRetry: (() -> Void)? = nil

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            LXIcon(name: .warning, size: 16, color: t.danger, stroke: 1.8)
                .frame(width: 20, height: 20)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text("当前离线")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(t.text)
                Text("网络不可用，新消息将无法送达。")
                    .font(.caption)
                    .foregroundColor(t.text2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if let onRetry {
                Button(action: onRetry) {
                    Text("重试")
                        .font(.caption.weight(.semibold))
                        .foregroundColor(.white)
                        .padding(.horizontal, 12).padding(.vertical, 6)
                        .background(t.danger)
                        .clipShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("重试连接")
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(t.danger.opacity(0.10))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.danger.opacity(0.40), lineWidth: 0.5))
        .accessibilityElement(children: .combine)
    }
}
