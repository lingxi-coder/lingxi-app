import SwiftUI
import WebKit

// MARK: - shared pieces

/// A cover in the market's single-character style, tinted by category.
struct AppCover: View {
    let name: String
    let category: String
    var size: CGFloat = 56

    private var color: Color {
        switch category {
        case "tools": Color(red: 0.43, green: 0.59, blue: 1.0)
        case "productivity": Color(red: 0.0, green: 0.76, blue: 0.77)
        case "games": Color(red: 0.81, green: 0.46, blue: 0.89)
        case "education": Color(red: 0.87, green: 0.58, blue: 0.0)
        case "visualization": Color(red: 0.90, green: 0.41, blue: 0.48)
        default: Color(red: 0.54, green: 0.56, blue: 0.60)
        }
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: size * 0.26, style: .continuous)
        Text(AppsModel.initial(name))
            .font(.system(size: size * 0.42, weight: .bold))
            .foregroundStyle(color)
            .frame(width: size, height: size)
            .background(color.opacity(0.16), in: shape)
            .overlay(shape.stroke(color.opacity(0.35), lineWidth: 0.5))
            .accessibilityHidden(true)
    }
}

func appCategoryLabel(_ category: String) -> String {
    switch category {
    case "tools": String(localized: "apps_category_tools")
    case "productivity": String(localized: "apps_category_productivity")
    case "games": String(localized: "apps_category_games")
    case "education": String(localized: "apps_category_education")
    case "visualization": String(localized: "apps_category_visualization")
    default: String(localized: "apps_category_other")
    }
}

@MainActor
private func perform(
    _ action: AppAction,
    app: MarketApp?,
    appId: String,
    market: LocalAppMarket,
    info: InstalledAppInfoStore,
    onOpenApp: (String) -> Void
) {
    switch action {
    case .get:
        guard let app, let version = app.currentVersion else { return }
        info.remember(app)
        market.install(appId: app.id, versionId: version.id)
    case .update(let versionID):
        if let app { info.remember(app) }
        // Installing a newer version over an installed one is the update.
        market.install(appId: appId, versionId: versionID)
    case .open:
        onOpenApp(appId)
    case .downloading:
        market.cancel(appId)
    case .installing, .removing, .unavailable:
        break
    }
}

/// Uninstalls, then forgets what the pages knew about the app once it is gone.
@MainActor
func uninstallApp(_ appId: String, market: LocalAppMarket, info: InstalledAppInfoStore) {
    Task { @MainActor in
        do {
            try await market.uninstall(appId)
            info.forget(appId)
        } catch {
            // The market keeps the failure in `operations`; the app's page shows it.
        }
    }
}

/// Deletes an app made in chat (its `chat:<id>` key), then forgets what the
/// pages knew about it once it is gone.
@MainActor
func deleteChatApp(_ key: String, chat: ChatApps, info: InstalledAppInfoStore) {
    guard let id = AppsModel.chatAppID(of: key) else { return }
    Task { @MainActor in
        if await chat.delete(id) { info.forget(key) }
    }
}

private func actionTitle(_ action: AppAction) -> String {
    switch action {
    case .get: String(localized: "apps_action_get")
    case .open: String(localized: "common_open")
    case .update: String(localized: "apps_action_update")
    case .downloading: String(localized: "apps_status_downloading")
    case .installing: String(localized: "apps_status_installing")
    case .removing: String(localized: "apps_status_removing")
    case .unavailable: String(localized: "apps_no_version")
    }
}

/// The compact button of a market row: a capsule, or a ring while busy (tap the ring to cancel).
private struct AppActionPill: View {
    @Environment(\.theme) private var t
    let action: AppAction
    let onTap: () -> Void

    var body: some View {
        switch action {
        case .downloading(let progress):
            Button(action: onTap) {
                ZStack {
                    Circle().stroke(t.border, lineWidth: 2.5)
                    Circle().trim(from: 0, to: progress).stroke(t.accent, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                    RoundedRectangle(cornerRadius: 1.5).fill(t.accent).frame(width: 8, height: 8)
                }
                .frame(width: 26, height: 26)
                .padding(7)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "common_cancel"))
        case .installing, .removing:
            ProgressView().controlSize(.small)
        case .unavailable:
            EmptyView()
        case .get, .open, .update:
            let filled = action != .open
            Button(action: onTap) {
                Text(actionTitle(action))
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(filled ? Color.white : t.accent)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 7)
                    .background(filled ? t.accent : t.accent.opacity(0.14), in: Capsule())
            }
            .buttonStyle(.plain)
        }
    }
}

// MARK: - 应用 page

enum AppsTab: String, Hashable { case mine, market }

/// 应用: the installed apps and the apps made in chat (我的), and the Local App
/// Market (市场). Opening an app runs it full screen; long-press a tile for its
/// details, update and uninstall, or to delete an app made in chat.
struct AppsPage: View {
    @Environment(\.theme) private var t
    let market: LocalAppMarket?
    let chat: ChatApps
    @ObservedObject var info: InstalledAppInfoStore
    let onOpenApp: (String) -> Void
    let onShowDetails: (String) -> Void

    @State private var tab: AppsTab = .mine

    var body: some View {
        Group {
            if let market {
                AppsPageContent(
                    market: market,
                    chat: chat,
                    info: info,
                    tab: $tab,
                    onOpenApp: onOpenApp,
                    onShowDetails: onShowDetails
                )
            } else {
                Text(String(localized: "apps_market_error \("UNAVAILABLE")"))
                    .foregroundStyle(t.text3)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(t.windowBg.ignoresSafeArea())
        .navigationTitle(String(localized: "apps_title"))
        .navigationBarTitleDisplayMode(.inline)
        .accessibilityIdentifier("apps.page")
    }
}

private struct AppsPageContent: View {
    @Environment(\.theme) private var t
    @ObservedObject var market: LocalAppMarket
    let chat: ChatApps
    @ObservedObject var info: InstalledAppInfoStore
    @Binding var tab: AppsTab
    let onOpenApp: (String) -> Void
    let onShowDetails: (String) -> Void

    @State private var query = ""
    @State private var searched = false
    @State private var lastSearch: String?
    /// The app to uninstall, or to delete when it was made in chat.
    @State private var uninstalling: MyApp?

    private var apps: [MyApp] { market.myApps(info: info.apps, chatApps: chat.apps) }

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $tab) {
                Text("apps_tab_mine").tag(AppsTab.mine)
                Text("apps_tab_market").tag(AppsTab.market)
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .accessibilityIdentifier("apps.tabs")
            switch tab {
            case .mine: mine
            case .market: marketList
            }
        }
        .task(id: market.installed.isEmpty) {
            if !market.installed.isEmpty { await market.checkUpdates() }
        }
        // The agent may have made or changed apps since the list was read.
        .task { await chat.refresh() }
        .task(id: "\(tab.rawValue)|\(query)") {
            // The market loads when first shown and follows the search as the
            // user types; a result already on screen for the same text stays.
            guard tab == .market else { return }
            let text = query.trimmingCharacters(in: .whitespacesAndNewlines)
            if text == lastSearch && (!market.apps.isEmpty || market.isLoading) { return }
            if searched {
                try? await Task.sleep(for: .milliseconds(300))
                guard !Task.isCancelled else { return }
            }
            searched = true
            lastSearch = text
            await market.search(query)
        }
        .confirmationDialog(
            uninstalling?.source == .chat
                ? String(localized: "apps_delete_title \(uninstalling?.name ?? "")")
                : String(localized: "apps_uninstall_title \(uninstalling?.name ?? "")"),
            isPresented: Binding(get: { uninstalling != nil }, set: { if !$0 { uninstalling = nil } }),
            titleVisibility: .visible,
            presenting: uninstalling
        ) { app in
            if app.source == .chat {
                Button(String(localized: "common_delete"), role: .destructive) {
                    deleteChatApp(app.appId, chat: chat, info: info)
                }
            } else {
                Button(String(localized: "apps_action_uninstall"), role: .destructive) {
                    uninstallApp(app.appId, market: market, info: info)
                }
            }
            Button("common_cancel", role: .cancel) {}
        } message: { _ in
            Text("apps_uninstall_body")
        }
    }

    @ViewBuilder
    private var mine: some View {
        ScrollView {
            if apps.isEmpty {
                VStack(alignment: .leading, spacing: 14) {
                    Text("apps_mine_empty").foregroundStyle(t.text3)
                    Button { tab = .market } label: {
                        Text("apps_browse_market")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 18)
                            .padding(.vertical, 11)
                            .background(t.accent, in: RoundedRectangle(cornerRadius: 12))
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("apps.browse-market")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
            } else {
                HStack {
                    Text("\(String(localized: "drawer_filter_all")) · \(apps.count)")
                        .font(.footnote.weight(.medium))
                        .foregroundStyle(t.text3)
                    Spacer()
                    Button("apps_check_updates") { Task { await market.checkUpdates() } }
                        .font(.footnote.weight(.medium))
                        .accessibilityIdentifier("apps.check-updates")
                }
                .padding(.horizontal, 16)
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 4), spacing: 14) {
                    ForEach(apps) { app in tile(app) }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            }
        }
    }

    private func tile(_ app: MyApp) -> some View {
        Button { if app.activeOperation == nil { onOpenApp(app.appId) } } label: {
            VStack(spacing: 6) {
                ZStack(alignment: .topTrailing) {
                    AppCover(name: app.name, category: app.category, size: 56)
                        .overlay {
                            if app.activeOperation != nil {
                                RoundedRectangle(cornerRadius: 14).fill(t.windowBg.opacity(0.55))
                                ProgressView()
                            }
                        }
                    if app.updateAvailable && app.activeOperation == nil {
                        Circle().fill(t.danger).frame(width: 9, height: 9)
                            .overlay(Circle().stroke(t.windowBg, lineWidth: 2))
                            .offset(x: 3, y: -3)
                    }
                }
                Text(app.name)
                    .font(.system(size: 12))
                    .foregroundStyle(t.text2)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(app.name)
        .accessibilityIdentifier("apps.tile.\(app.appId)")
        .contextMenu {
            // An app made in chat has no market details, updates or install.
            if app.source == .chat {
                Button(role: .destructive) { uninstalling = app } label: {
                    Label("common_delete", systemImage: "trash")
                }
            } else {
                marketTileMenu(app)
            }
        }
    }

    @ViewBuilder
    private func marketTileMenu(_ app: MyApp) -> some View {
        Button { onShowDetails(app.appId) } label: { Label("apps_details", systemImage: "info.circle") }
        if app.activeOperation == nil {
            if app.updateAvailable {
                Button {
                    if let version = market.updates.first(where: { $0.appId == app.appId })?.currentVersionId {
                        market.install(appId: app.appId, versionId: version)
                    }
                } label: { Label("apps_action_update", systemImage: "arrow.down.circle") }
            }
            Button(role: .destructive) { uninstalling = app } label: {
                Label("apps_action_uninstall", systemImage: "trash")
            }
        }
    }

    private var marketList: some View {
        List {
            Section {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").foregroundStyle(t.text4)
                    TextField("apps_search_hint", text: $query)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("apps.search")
                }
            }
            Section {
                ForEach(market.apps) { app in
                    HStack(spacing: 12) {
                        Button { onShowDetails(app.id) } label: {
                            HStack(spacing: 12) {
                                AppCover(name: app.name, category: app.category, size: 48)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(app.name).font(.system(size: 15, weight: .semibold)).foregroundStyle(t.text).lineLimit(1)
                                    if !app.summary.isEmpty {
                                        Text(app.summary).font(.footnote).foregroundStyle(t.text3).lineLimit(2)
                                    }
                                    Text(appCategoryLabel(app.category)).font(.caption2).foregroundStyle(t.text4)
                                }
                                Spacer(minLength: 4)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        let action = market.action(for: app.id, current: app.currentVersion)
                        AppActionPill(action: action) {
                            perform(action, app: app, appId: app.id, market: market, info: info, onOpenApp: onOpenApp)
                        }
                    }
                    .accessibilityIdentifier("apps.market.\(app.id)")
                }
                footer
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
    }

    @ViewBuilder
    private var footer: some View {
        if market.isLoading {
            ProgressView().frame(maxWidth: .infinity)
        } else if let error = market.error {
            VStack(spacing: 8) {
                Text(String(localized: "apps_market_error \(error)"))
                    .font(.footnote)
                    .foregroundStyle(t.text3)
                    .multilineTextAlignment(.center)
                Button("common_retry") {
                    Task {
                        // A failed first page starts over; a failed later page resumes.
                        if market.apps.isEmpty { await market.search(query) } else { await market.loadNextPage() }
                    }
                }
                .accessibilityIdentifier("apps.market.retry")
            }
            .frame(maxWidth: .infinity)
        } else if market.hasNextPage {
            // The end of the list came into view: fetch the next page.
            ProgressView().frame(maxWidth: .infinity).task { await market.loadNextPage() }
        } else if searched && market.apps.isEmpty {
            Text("apps_market_empty").font(.footnote).foregroundStyle(t.text4).frame(maxWidth: .infinity)
        }
    }
}

// MARK: - one app's page

/// One app: what the market says about it, the button that gets, opens,
/// updates or cancels it, its versions, and uninstall. An installed app keeps
/// its remembered name and can be opened or removed while the market is down.
struct MarketAppPage: View {
    @Environment(\.theme) private var t
    @ObservedObject var market: LocalAppMarket
    @ObservedObject var info: InstalledAppInfoStore
    let appId: String
    let onOpenApp: (String) -> Void

    @State private var app: MarketApp?
    @State private var versions: [MarketVersion] = []
    @State private var loadError: String?
    @State private var attempt = 0
    @State private var confirmUninstall = false

    private var installed: Bool { market.installed.contains { $0.appId == appId && !$0.removing } }
    private var name: String { app?.name ?? info.apps[appId]?.name ?? appId }
    private var category: String { app?.category ?? info.apps[appId]?.category ?? "" }
    private var summary: String { app?.summary ?? info.apps[appId]?.summary ?? "" }

    var body: some View {
        let action = market.action(for: appId, current: app?.currentVersion)
        List {
            Section {
                HStack(spacing: 14) {
                    AppCover(name: name, category: category, size: 72)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(name).font(.title3.weight(.bold)).foregroundStyle(t.text).lineLimit(2)
                        Text(([appCategoryLabel(category)] + (installed ? [String(localized: "apps_installed")] : [])).joined(separator: " · "))
                            .font(.footnote)
                            .foregroundStyle(t.text4)
                    }
                }
                if !summary.isEmpty {
                    Text(summary).font(.subheadline).foregroundStyle(t.text2)
                }
                mainButton(action)
                if case .failed(let message)? = market.operations[appId].flatMap(AppOperation.init) {
                    Text(String(localized: "apps_operation_failed \(message)"))
                        .font(.footnote)
                        .foregroundStyle(t.danger)
                }
                if app == nil && loadError == nil {
                    ProgressView().frame(maxWidth: .infinity)
                }
                if let loadError {
                    HStack {
                        Text(String(localized: "apps_market_error \(loadError)")).font(.footnote).foregroundStyle(t.text3)
                        Spacer()
                        Button("common_retry") { attempt += 1 }
                    }
                }
            }
            if let version = app?.currentVersion {
                Section(String(localized: "apps_version_notes")) {
                    versionRow(version)
                }
            }
            let older = versions.filter { $0.id != app?.currentVersion?.id }
            if !older.isEmpty {
                Section(String(localized: "apps_versions")) {
                    ForEach(older) { versionRow($0) }
                }
            }
            if installed && action != .removing {
                Section {
                    Button(role: .destructive) { confirmUninstall = true } label: {
                        Text("apps_action_uninstall")
                    }
                    .accessibilityIdentifier("market-app.uninstall")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(t.windowBg.ignoresSafeArea())
        .navigationTitle(String(localized: "apps_details"))
        .navigationBarTitleDisplayMode(.inline)
        .task(id: attempt) { await load() }
        .confirmationDialog(
            String(localized: "apps_uninstall_title \(name)"),
            isPresented: $confirmUninstall,
            titleVisibility: .visible
        ) {
            Button(String(localized: "apps_action_uninstall"), role: .destructive) {
                uninstallApp(appId, market: market, info: info)
            }
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("apps_uninstall_body")
        }
        .accessibilityIdentifier("market-app.page")
    }

    private func load() async {
        loadError = nil
        do {
            let loaded = try await market.detail(appId)
            app = loaded
            if installed { info.remember(loaded) }
            versions = try await market.versions(appId).data
        } catch is CancellationError {
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func versionRow(_ version: MarketVersion) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(String(localized: "apps_version \(version.sequence)")).foregroundStyle(t.text)
                Spacer()
                if let size = version.sizeBytes {
                    Text(ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file))
                        .font(.footnote)
                        .foregroundStyle(t.text4)
                }
            }
            if !version.notes.isEmpty {
                Text(version.notes).font(.footnote).foregroundStyle(t.text3)
            }
        }
    }

    @ViewBuilder
    private func mainButton(_ action: AppAction) -> some View {
        let run = { perform(action, app: app, appId: appId, market: market, info: info, onOpenApp: onOpenApp) }
        switch action {
        case .downloading(let progress):
            Button(action: run) {
                VStack(spacing: 10) {
                    HStack {
                        Text("apps_status_downloading").foregroundStyle(t.text2)
                        Spacer()
                        Text("common_cancel").fontWeight(.semibold).foregroundStyle(t.accent)
                    }
                    ProgressView(value: progress).tint(t.accent)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("market-app.action")
        default:
            let enabled = action == .get || action == .open || { if case .update = action { true } else { false } }()
            Button(action: run) {
                HStack(spacing: 8) {
                    if action == .installing || action == .removing { ProgressView().controlSize(.small) }
                    Text(actionTitle(action)).font(.system(size: 15, weight: .semibold))
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
                .foregroundStyle(enabled ? Color.white : t.text3)
                .background(enabled ? t.accent : t.surface, in: RoundedRectangle(cornerRadius: 12))
            }
            .buttonStyle(.plain)
            .disabled(!enabled)
            .accessibilityIdentifier("market-app.action")
        }
    }
}

// MARK: - running app

/// An installed app running full screen. The market's runtime, or for an app
/// made in chat (`appId` `chat:<id>`) the chat runtime, owns the web view (its
/// own origin and persistent store, no native bridge); this view only hosts
/// it, pauses it with the scene and closes it on the way out. The only chrome
/// is the capsule in the top-right corner, as in a mini program.
struct AppRuntimeView: View {
    @Environment(\.theme) private var t
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.openURL) private var openURL
    let market: LocalAppMarket
    let chat: ChatApps
    let info: InstalledAppInfoStore
    let appId: String
    let onClose: () -> Void
    let onShowDetails: (String) -> Void

    @State private var webView: WKWebView?
    @State private var openError: String?
    @State private var externalURL: URL?

    private var chatID: String? { AppsModel.chatAppID(of: appId) }

    var body: some View {
        ZStack(alignment: .topTrailing) {
            t.windowBg.ignoresSafeArea()
            if let webView {
                HostedWebView(webView: webView)
            } else if let openError {
                VStack(spacing: 14) {
                    Text(String(localized: "apps_open_failed \(openError)"))
                        .foregroundStyle(t.text2)
                        .multilineTextAlignment(.center)
                    Button("common_close", action: onClose)
                }
                .padding(32)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            capsule.padding(.top, 6).padding(.trailing, 10)
        }
        .task(id: appId) {
            let link = $externalURL
            let onExternalNavigation: (URL) -> Void = { url in link.wrappedValue = url }
            do {
                if let chatID {
                    chat.runtime.onExternalNavigation = onExternalNavigation
                    webView = try await chat.runtime.open(chatID)
                } else {
                    market.runtime.onExternalNavigation = onExternalNavigation
                    webView = try await market.open(appId)
                }
                info.markOpened(appId)
            } catch {
                openError = error.localizedDescription
            }
        }
        .onDisappear {
            // Release the view before closing: the runtime cannot delete an
            // app's store while a consumer still holds its web view.
            webView = nil
            if let chatID {
                chat.runtime.onExternalNavigation = nil
                Task { await chat.runtime.close(chatID) }
            } else {
                market.runtime.onExternalNavigation = nil
                Task { await market.close(appId) }
            }
        }
        .onChange(of: scenePhase) { _, phase in
            if let chatID {
                if phase == .active { chat.runtime.resume(chatID) } else { chat.runtime.suspend(chatID) }
            } else {
                if phase == .active { market.resume(appId) } else { market.suspend(appId) }
            }
        }
        .alert(
            String(localized: "apps_external_link_title"),
            isPresented: Binding(get: { externalURL != nil }, set: { if !$0 { externalURL = nil } }),
            presenting: externalURL
        ) { url in
            Button("common_open") { openURL(url) }
            Button("common_cancel", role: .cancel) {}
        } message: { url in
            Text(String(localized: "apps_external_link_body \(url.absoluteString)"))
        }
        .accessibilityIdentifier("app-runtime")
    }

    private var capsule: some View {
        HStack(spacing: 0) {
            // An app made in chat has no market details to show.
            if chatID == nil {
                Menu {
                    Button { onShowDetails(appId) } label: { Label("apps_details", systemImage: "info.circle") }
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 15, weight: .bold))
                        .frame(width: 46, height: 32)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel(String(localized: "composer_more"))
                .accessibilityIdentifier("app-runtime.more")
                Rectangle().fill(t.border).frame(width: 0.5, height: 16)
            }
            Button(action: onClose) {
                Image(systemName: "xmark")
                    .font(.system(size: 14, weight: .semibold))
                    .frame(width: 46, height: 32)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel(String(localized: "common_close"))
            .accessibilityIdentifier("app-runtime.close")
        }
        .foregroundStyle(t.text)
        .background(t.surface.opacity(0.92), in: Capsule())
        .overlay(Capsule().stroke(t.border, lineWidth: 0.5))
    }
}

/// Hosts the runtime's web view inside a container, so SwiftUI never owns or
/// recreates the view itself.
private struct HostedWebView: UIViewRepresentable {
    let webView: WKWebView

    func makeUIView(context: Context) -> UIView {
        let container = UIView()
        embed(in: container)
        return container
    }

    func updateUIView(_ container: UIView, context: Context) {
        if webView.superview !== container { embed(in: container) }
    }

    static func dismantleUIView(_ container: UIView, coordinator: ()) {
        container.subviews.forEach { $0.removeFromSuperview() }
    }

    private func embed(in container: UIView) {
        container.subviews.forEach { $0.removeFromSuperview() }
        webView.frame = container.bounds
        webView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        container.addSubview(webView)
    }
}
