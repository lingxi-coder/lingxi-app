import SwiftUI
import WebKit
import UniformTypeIdentifiers

private struct LocalAppSourceDocument: FileDocument {
    static var readableContentTypes: [UTType] { [.zip] }
    let bytes: Data
    init(bytes: Data) { self.bytes = bytes }
    init(configuration: ReadConfiguration) throws {
        guard let data = configuration.file.regularFileContents else { throw MarketFailure("INVALID_ZIP") }
        bytes = data
    }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: bytes) }
}

struct AppCover: View {
    let name: String
    let category: String
    var size: CGFloat = 56
    var body: some View {
        Text(AppsModel.initial(name)).font(.system(size:size * 0.42,weight:.semibold))
            .foregroundStyle(.tint).frame(width:size,height:size)
            .background(Color.accentColor.opacity(0.12),in:.rect(cornerRadius:size * 0.24)).accessibilityHidden(true)
    }
}

enum AppsTab: String, Hashable { case mine, market }
struct LocalAppDevelopmentRequest: Sendable {
    let appId: String?
    let name: String
    let sourceDirectory: String
    let sourceSessionID: String?
    let prompt: String
}

struct AppsPage: View {
    @Environment(\.theme) private var t
    let library: UnifiedLocalApps
    let market: UnifiedAppMarket
    let onOpenApp: (String) -> Void
    let onShowDetails: (String) -> Void
    let onShowMarketDetails: (String) -> Void
    let onDevelop: (LocalAppDevelopmentRequest) -> Void
    @State private var tab: AppsTab = .mine
    @State private var query = ""
    @State private var creating = false
    @State private var importing = false
    @State private var importingLegacy: LocalAppLegacyEntry?
    @State private var consent: MarketConsent?
    @State private var error: String?
    private struct MarketConsent: Identifiable { var id: String { distribution.versionId }; let distribution: MarketDistribution; let remix: Bool }
    private var filtered: [UnifiedAppRecord] { library.records.filter { query.isEmpty || $0.name.localizedStandardContains(query) } }
    var body: some View {
        VStack(spacing:0) {
            Picker("Apps",selection:$tab) { Text("我的").tag(AppsTab.mine); Text("市场").tag(AppsTab.market) }
                .pickerStyle(.segmented).padding()
            List {
                if let message = error ?? library.error { Text(message).foregroundStyle(.red).textSelection(.enabled) }
                if tab == .mine {
                    ForEach(filtered) { app in
                        Button { onShowDetails(app.appId) } label: {
                            HStack(spacing:12) {
                                AppCover(name:app.name,category:"",size:46)
                                VStack(alignment:.leading,spacing:4) { Text(app.name).foregroundStyle(t.text); Text(library.runs[app.appId]?.phase ?? "stopped").font(.caption).foregroundStyle(t.text3) }
                                Spacer(); Button("打开") { onOpenApp(app.appId) }.buttonStyle(.borderless)
                            }.padding(.vertical,4)
                        }
                    }
                    if filtered.isEmpty { ContentUnavailableView("还没有应用",systemImage:"square.grid.2x2",description:Text("创建应用或从市场安装。应用独立于开发会话保存。")) }
                    if !library.legacyArchives.isEmpty {
                        Section("旧应用数据") {
                            Text("旧包和浏览器数据已保留。显式导入会创建新的应用和浏览器空间，不会自动迁移浏览器数据库。").font(.footnote).foregroundStyle(.secondary)
                            ForEach(library.legacyEntries) { entry in Button("导入 \(entry.name)") { importingLegacy = entry } }
                            ForEach(library.legacyArchives) { archive in Text(URL(fileURLWithPath:archive.path).lastPathComponent).font(.caption) }
                            Button("导入源码文件夹") { importing = true }
                        }
                    }
                } else {
                    if let error = market.error { Text(error).foregroundStyle(.red) }
                    ForEach(market.apps) { app in
                        VStack(alignment:.leading,spacing:10) {
                            HStack { AppCover(name:app.name,category:app.category,size:44); VStack(alignment:.leading) { Text(app.name).font(.headline); Text(app.summary).font(.caption).foregroundStyle(.secondary) } }
                            HStack {
                                Button("详情") { onShowMarketDetails(app.id) }
                                Button(library.records.contains(where: { $0.marketOrigin?.appId == app.id && $0.marketOrigin?.remixedFrom == nil }) ? "检查更新" : "获取") { loadDistribution(app.id,remix:false) }
                                if app.currentVersion?.manifest?.remix?.allowed == true { Button("制作可编辑副本") { loadDistribution(app.id,remix:true) } }
                            }.buttonStyle(.borderless)
                        }.padding(.vertical,6)
                    }
                    if market.loading { ProgressView() }
                    if let progress = market.progress { ProgressView(value:progress) }
                    if market.hasNextPage { Button("加载更多") { Task { await market.nextPage() } } }
                }
            }.listStyle(.plain)
        }
        .background(t.windowBg)
        .navigationTitle("应用").navigationBarTitleDisplayMode(.inline)
        .searchable(text:$query,prompt:"搜索应用")
        .toolbar { ToolbarItem(placement:.topBarTrailing) { Menu { Button("创建应用") { creating = true }; Button("导入源码文件夹") { importing = true } } label: { Image(systemName:"plus") }.accessibilityLabel("创建或导入应用") } }
        .task { await library.refresh() }
        .task(id:"\(tab.rawValue):\(query)") {
            guard tab == .market else { return }
            do { try await Task.sleep(for:.milliseconds(250)); try Task.checkCancellation(); await market.search(query) } catch { }
        }
        .sheet(isPresented:$creating) { LocalAppCreateSheet(onCreate:onDevelop) }
        .sheet(item:$consent) { item in
            LocalAppConsentSheet(manifest:item.distribution.manifest,digest:item.distribution.releaseDigest,title:item.remix ? "创建可编辑副本" : "安装此版本", onCancel: { market.cancelInstall() }, canCancel: !market.installCommitting) {
                Task {
                    do {
                        let app = try await market.install(item.distribution,approved:true,remix:item.remix)
                        consent = nil
                        if item.remix, let authoring = app.authoring { onDevelop(.init(appId:app.appId,name:app.name,sourceDirectory:authoring.sourceDirectory,sourceSessionID:nil,prompt:"继续开发这个应用。")) }
                    } catch { self.error = error.localizedDescription; consent = nil }
                }
            }
        }
        .confirmationDialog("导入旧应用？", isPresented: Binding(get: { importingLegacy != nil }, set: { if !$0 { importingLegacy = nil } }), presenting: importingLegacy) { entry in
            Button("创建新应用副本") { Task { do { let app = try await library.importLegacy(entry); importingLegacy = nil; onShowDetails(app.appId) } catch { self.error = error.localizedDescription } } }
            Button("取消", role: .cancel) { importingLegacy = nil }
        } message: { _ in Text("原包和浏览器数据保持原样。新副本使用新应用身份，不会复制浏览器数据库或原执行授权。") }
        .fileImporter(isPresented:$importing,allowedContentTypes:[.folder, .zip]) { result in
            Task {
                do {
                    let url = try result.get(); let scoped = url.startAccessingSecurityScopedResource(); defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                    let imported = try await library.importSource(url)
                    await library.refresh(); onShowDetails(imported.record.appId)
                } catch { self.error = error.localizedDescription }
            }
        }
        .accessibilityIdentifier("apps.page")
    }
    private func loadDistribution(_ id: String, remix: Bool) {
        Task { do { consent = .init(distribution:try await market.distribution(id),remix:remix) } catch { self.error = error.localizedDescription } }
    }
}

private struct LocalAppCreateSheet: View {
    @Environment(\.dismiss) private var dismiss
    let onCreate: (LocalAppDevelopmentRequest) -> Void
    @State private var name = ""
    @State private var prompt = ""
    @State private var sourceDirectory = ""
    @State private var error: String?
    var body: some View {
        NavigationStack {
            Form {
                TextField("应用名称",text:$name)
                TextField("已有源码目录（可选）", text: $sourceDirectory).textInputAutocapitalization(.never).autocorrectionDisabled()
                Text("留空会在 ~/.lingxi/.apps/ 创建目录；已有源码请填写 Linux 用户目录。").font(.footnote).foregroundStyle(.secondary)
                TextField("希望应用完成什么？",text:$prompt,axis:.vertical).lineLimit(3...8)
                Text("源码保存在本机 Linux 用户空间。创建后进入普通开发会话；本地发布与发布到市场是不同操作。").font(.footnote).foregroundStyle(.secondary)
                if let error { Text(error).foregroundStyle(.red) }
            }
            .navigationTitle("创建应用")
            .toolbar {
                ToolbarItem(placement:.cancellationAction) { Button("取消") { dismiss() } }
                ToolbarItem(placement:.confirmationAction) { Button("开始开发") {
                    do {
                        let directory: String
                        let selected = sourceDirectory.trimmingCharacters(in: .whitespacesAndNewlines)
                        if selected.isEmpty { directory = try UnifiedLocalApps.newAuthoringDirectory(name:name) }
                        else {
                            let url = try UnifiedLocalApps.hostSourceDirectory(selected)
                            var isDirectory: ObjCBool = false
                            guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory), isDirectory.boolValue else { throw LocalAppServiceFailure("SOURCE_MISSING", "请选择已存在的 Linux 用户目录。") }
                            directory = selected
                        }
                        onCreate(.init(appId:nil,name:name,sourceDirectory:directory,sourceSessionID:nil,prompt:prompt)); dismiss()
                    } catch { self.error = error.localizedDescription }
                }.disabled(name.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty) }
            }
        }
    }
}

struct LocalAppConsentSheet: View {
    @Environment(\.dismiss) private var dismiss
    let manifest: UnifiedAppManifest
    let digest: String
    let title: String
    var onCancel: (() -> Void)? = nil
    var canCancel = true
    let onApprove: () -> Void
    @State private var submitted = false
    var body: some View {
        NavigationStack {
            Form {
                Section(manifest.name) {
                    Text(manifest.summary ?? "")
                    if manifest.executesCode { Text("安装、构建和服务命令将在你本机的 Linux 用户权限下执行，可以访问该用户空间。每个应用的工作目录不是安全沙箱。") }
                    Text("请求能力：" + (manifest.capabilities ?? []).joined(separator:", "))
                    Text("授权绑定当前发布内容；修改执行内容需要重新授权。系统设备权限仍需单独允许。").font(.footnote)
                }
                Section("发布摘要") { Text(digest).font(.caption.monospaced()).textSelection(.enabled) }
                if let remix = manifest.remix, remix.allowed { Section("修改许可") { Text(remix.license ?? "") } }
            }
            .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
            .interactiveDismissDisabled(submitted)
            .toolbar {
                ToolbarItem(placement:.cancellationAction) { Button("取消") { onCancel?(); dismiss() }.disabled(!canCancel) }
                ToolbarItem(placement:.confirmationAction) { Button("允许") { submitted = true; onApprove() }.disabled(submitted) }
            }
        }
    }
}

struct LocalAppDetailPage: View {
    let library: UnifiedLocalApps
    let market: UnifiedAppMarket
    let appId: String
    let onOpenApp: (String) -> Void
    let onDevelop: (UnifiedAppRecord) -> Void
    @State private var releases: [UnifiedAppRelease] = []
    @State private var logs = ""
    @State private var dataPath = ""
    @State private var error: String?
    @State private var consent: UnifiedAppRelease?
    @State private var deleting = false
    @State private var publication: LocalAppMarketPublication?
    @State private var preparingPublication = false
    @State private var exportingSource = false
    @State private var sourceDocument: LocalAppSourceDocument?
    @State private var restoring = false
    @State private var clearingBrowserData = false
    var body: some View {
        List {
            if let app = library.record(appId) {
                Section {
                    Text(app.name).font(.title2.weight(.semibold)); Text(app.summary).foregroundStyle(.secondary)
                    LabeledContent("状态",value:library.isPaused(appId) ? "已暂停，等待恢复" : (library.runs[appId]?.phase ?? "stopped"))
                    Button(library.isPaused(appId) ? "恢复运行" : "运行") { requestRun(app) }
                    Button("停止") { operation("stop") }.disabled(library.runs[appId]?.isLive != true)
                    Button("继续开发") { onDevelop(app) }
                    if app.marketOrigin == nil { Button("从当前版本恢复源码副本") { restoring = true } }
                    Button("修复依赖并重试") { operation("repair") }
                }
                Section("版本") {
                    ForEach(releases) { release in
                        HStack {
                            Text("版本 \(release.revision)"); Spacer()
                            if release.revision == app.currentRevision { Text("当前").foregroundStyle(.secondary) }
                            else { Button("运行此版本") { requestRelease(release, app: app) } }
                        }
                    }
                }
                Section("权限") {
                    Text("执行授权按发布摘要保存；撤销会停止当前应用。").font(.footnote)
                    Button("撤销执行授权",role:.destructive) { operation("revoke") }
                    if let release = releases.first(where: { $0.revision == app.currentRevision }) {
                        ForEach((release.manifest.capabilities ?? []).filter { LocalAppDevicePermissions.supported.contains($0) }, id: \.self) { capability in
                            Toggle(capability, isOn: Binding(get: {
                                library.devicePermissions.allows(capability, appID: appId, digest: release.digest)
                            }, set: { value in
                                library.devicePermissions.set(capability, allowed: value, appID: appId, digest: release.digest)
                                if !value { library.browser.close(appId) }
                            }))
                        }
                    }
                    Text("设备能力另行授权，只对当前发布有效；系统权限仍单独管理。").font(.footnote)
                    Button("撤销全部设备授权", role: .destructive) {
                        library.devicePermissions.revoke(appID: appId); library.browser.close(appId)
                    }
                }
                Section("数据") {
                    Text(dataPath).font(.caption.monospaced()).textSelection(.enabled)
                    Text("应用数据独立于源码与发布版本。更换账号不会切换或删除本机用户空间。").font(.footnote)
                    Button("导出当前版本源码 ZIP") { Task { do {
                        let source = try await library.sourceArchive(appId, revision: app.currentRevision)
                        sourceDocument = .init(bytes: source.bytes); exportingSource = true
                    } catch { self.error = error.localizedDescription } } }
                    Text("源码包不包含运行数据库、浏览器数据、账号凭据或授权。").font(.footnote).foregroundStyle(.secondary)
                    Button("清除浏览器数据",role:.destructive) { clearingBrowserData = true }
                }
                Section("日志") { Text(logs.isEmpty ? "当前没有日志" : logs).font(.caption.monospaced()).textSelection(.enabled); Button("刷新日志") { Task { await load() } } }
                Section("发布") {
                    Button("发布到市场") { Task {
                        preparingPublication = true; defer { preparingPublication = false }
                        do { publication = try await market.preparePublication(app) } catch { self.error = error.localizedDescription }
                    } }.disabled(preparingPublication || market.publishing)
                    if preparingPublication { ProgressView("准备源码清单…") }
                    if let url = market.managementURL { Link("管理市场发行与审核", destination: url) }
                    if let status = market.publishStatus { Text(status).font(.footnote) }
                }
                Section { Button("移除应用",role:.destructive) { deleting = true } }
            } else { ContentUnavailableView("应用不可用",systemImage:"square.dashed",description:Text("应用可能已移除，旧数据不会自动删除。")) }
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
        }
        .navigationTitle("应用详情").navigationBarTitleDisplayMode(.inline)
        .task(id:appId) { await load() }
        .fileExporter(isPresented: $exportingSource, document: sourceDocument, contentType: .zip, defaultFilename: "local-app-source") { result in
            sourceDocument = nil
            if case .failure(let failure) = result { error = failure.localizedDescription }
        }
        .sheet(item: $publication) { prepared in
            NavigationStack {
                List {
                    Section(prepared.name) {
                        Text(prepared.summary)
                        Text("仅上传下列源码和锁定依赖，不包含用户数据。服务首发或权限扩大需人工审核。").font(.footnote)
                        Text(prepared.source.digest).font(.caption.monospaced()).textSelection(.enabled)
                    }
                    Section("公开文件清单") { ForEach(prepared.source.files, id: \.self) { Text($0).font(.caption.monospaced()) } }
                }
                .navigationTitle("审阅公开源码").navigationBarTitleDisplayMode(.inline)
                .interactiveDismissDisabled(market.publishing)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("取消") { publication = nil }.disabled(market.publishing) }
                    ToolbarItem(placement: .confirmationAction) { Button("发布") { Task { do { try await market.publish(prepared); publication = nil } catch { self.error = error.localizedDescription; publication = nil } } }.disabled(market.publishing) }
                }
            }
        }
        .sheet(item:$consent) { release in LocalAppConsentSheet(manifest:release.manifest,digest:release.digest,title:"授权此版本运行") { Task { do { try await library.grant(appId,digest:release.digest); consent = nil; await runRelease(release) } catch { self.error = error.localizedDescription; consent = nil } } } }
        .confirmationDialog("清除浏览器数据？", isPresented: $clearingBrowserData, titleVisibility: .visible) {
            Button("停止并清除浏览器数据", role: .destructive) { Task { do { try await library.stop(appId); try await library.browser.clearData(appId) } catch { self.error = error.localizedDescription } } }
            Button("取消", role: .cancel) { }
        } message: { Text("将清除这个应用的登录状态、localStorage 和 IndexedDB。服务数据目录保留。") }
        .confirmationDialog("恢复可编辑源码副本？", isPresented: $restoring, titleVisibility: .visible) {
            Button("恢复副本并继续开发") { Task { do { guard let app = library.record(appId) else { return }; onDevelop(try await library.restoreAuthoring(app)) } catch { self.error = error.localizedDescription } } }
            Button("取消", role: .cancel) { }
        } message: { Text("从当前不可变发布生成新的源码目录，原目录和发布版本保留。") }
        .confirmationDialog("移除应用？",isPresented:$deleting,titleVisibility:.visible) { Button("移除",role:.destructive) { operation("remove") }; Button("取消",role:.cancel) {} } message: { Text("应用将从列表移除。源码和旧归档不受影响。") }
    }
    private func requestRun(_ app: UnifiedAppRecord) {
        guard let release = releases.first(where: { $0.revision == app.currentRevision }) else { error = "当前版本尚未加载，请刷新后重试。"; return }
        requestRelease(release, app: app)
    }
    private func requestRelease(_ release: UnifiedAppRelease, app: UnifiedAppRecord) {
        if release.manifest.executesCode && !app.executionGrants.contains(release.digest) { consent = release }
        else { Task { await runRelease(release) } }
    }
    private func runRelease(_ release: UnifiedAppRelease) async {
        do {
            try await library.action("start", id: appId, extra: ["revision": .number(Double(release.revision))])
            onOpenApp(appId)
        } catch { self.error = error.localizedDescription; await library.refresh() }
    }
    private func operation(_ value: String) { Task { do { try await library.action(value,id:appId); await load() } catch { self.error = error.localizedDescription } } }
    private func load() async {
        do {
            await library.refresh(); releases = try await library.releases(appId)
            dataPath = try UnifiedLocalApps.guestDirectory(await library.layout(appId).dataPath)
            if let run = library.runs[appId] { logs = try await library.logs(appId,runID:run.runId) }
        } catch { self.error = error.localizedDescription }
    }
}

struct AppRuntimeView: View {
    @Environment(\.openURL) private var openURL
    let library: UnifiedLocalApps
    let info: InstalledAppInfoStore
    let appId: String
    let onClose: () -> Void
    let onShowDetails: (String) -> Void
    @State private var webView: WKWebView?
    @State private var error: String?
    @State private var externalURL: URL?
    @State private var consent: UnifiedAppRelease?
    var body: some View {
        VStack(spacing:0) {
            HStack {
                Text(library.record(appId)?.name ?? "应用").font(.headline).lineLimit(1); Spacer()
                Menu {
                    Button("应用详情", systemImage: "info.circle") { onShowDetails(appId) }
                    Button("刷新", systemImage: "arrow.clockwise") { webView?.reload() }
                    Button("停止", systemImage: "stop.circle", role: .destructive) {
                        Task { do { try await library.stop(appId); onClose() } catch { self.error = error.localizedDescription } }
                    }
                } label: { Image(systemName:"ellipsis") }.accessibilityLabel("应用菜单")
                Button(action:onClose) { Image(systemName:"xmark") }.accessibilityLabel("关闭应用窗口")
            }.padding()
            if library.isPaused(appId) {
                ContentUnavailableView { Label("应用已暂停",systemImage:"pause.circle") } description: { Text("系统后台运行时间已结束。返回应用后可恢复运行。")} actions: { Button("恢复") { Task { await load() } } }
            } else if let webView { HostedWebView(webView:webView) }
            else if let error { ContentUnavailableView { Label("无法运行",systemImage:"exclamationmark.triangle") } description: { Text(error) } actions: { Button("重试") { Task { await load() } }; Button("详情") { onShowDetails(appId) } } }
            else { ProgressView().frame(maxWidth:.infinity,maxHeight:.infinity) }
        }
        .task(id:appId) { await load() }
        // A picker or camera may present above this view. Its return must not
        // tear down the app, change navigation, or clear its document state.
        .sheet(item:$consent) { release in LocalAppConsentSheet(manifest:release.manifest,digest:release.digest,title:"授权此版本运行") { Task { do { try await library.grant(appId,digest:release.digest); consent = nil; await load() } catch { self.error = error.localizedDescription; consent = nil } } } }
        .alert("打开外部链接？",isPresented:Binding(get:{externalURL != nil},set:{if !$0 { externalURL = nil }}),presenting:externalURL) { url in Button("打开") { openURL(url) }; Button("取消",role:.cancel) {} } message: { url in Text(url.absoluteString) }
        .accessibilityIdentifier("app-runtime")
    }
    private func load() async {
        error = nil
        do {
            await library.refresh()
            guard let app = library.record(appId) else { throw LocalAppServiceFailure("APP_NOT_FOUND","The local app was not found.") }
            let release = try await library.release(appId,revision:app.currentRevision)
            if release.manifest.executesCode && !app.executionGrants.contains(release.digest) { consent = release; return }
            webView = try await library.browser.open(appId) { externalURL = $0 }; info.markOpened(app)
        } catch { self.error = error.localizedDescription }
    }
}

private struct HostedWebView: UIViewRepresentable {
    let webView: WKWebView
    func makeUIView(context:Context) -> UIView { let container = UIView(); attach(to:container); return container }
    func updateUIView(_ view:UIView,context:Context) { attach(to:view) }
    private func attach(to container:UIView) {
        guard webView.superview !== container else { return }
        webView.removeFromSuperview(); container.addSubview(webView); webView.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([webView.leadingAnchor.constraint(equalTo:container.leadingAnchor),webView.trailingAnchor.constraint(equalTo:container.trailingAnchor),webView.topAnchor.constraint(equalTo:container.topAnchor),webView.bottomAnchor.constraint(equalTo:container.bottomAnchor)])
    }
}

struct MarketAppDetailPage: View {
    let market: UnifiedAppMarket
    let appId: String
    let onOpenApp: (String) -> Void
    let onDevelop: (LocalAppDevelopmentRequest) -> Void
    @State private var app: MarketApp?
    @State private var distribution: MarketDistribution?
    @State private var error: String?
    @State private var consenting = false
    @State private var remix = false
    var body: some View {
        List {
            if let app {
                Section { AppCover(name: app.name, category: app.category); Text(app.name).font(.title2.weight(.semibold)); Text(app.summary) }
            }
            if let release = distribution {
                Section("此设备可用的版本") {
                    Text("版本 \(release.sequence) · \(release.manifest.executesCode ? "Linux 服务" : "静态 Web 应用")")
                    Text("源码 + \(release.dependencies.count) 个依赖锁定文件，在本机构建。")
                    Text("能力：" + (release.manifest.capabilities ?? []).joined(separator: ", "))
                    Text(release.releaseDigest).font(.caption.monospaced()).textSelection(.enabled)
                    Button("安装或更新此版本") { remix = false; consenting = true }
                }
                if let license = release.manifest.remix, license.allowed {
                    Section("可编辑副本") {
                        Text(license.license ?? "")
                        Text("副本拥有新的应用身份和源码目录，不跟随原应用更新。").font(.footnote)
                        Button("制作副本并继续开发") { remix = true; consenting = true }
                    }
                }
            }
            if let progress = market.progress { ProgressView(value: progress) }
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            Button("刷新") { Task { await load() } }
        }
        .navigationTitle("市场应用").navigationBarTitleDisplayMode(.inline)
        .task(id: appId) { await load() }
        .sheet(isPresented: $consenting) {
            if let distribution {
                LocalAppConsentSheet(manifest: distribution.manifest, digest: distribution.releaseDigest,
                    title: remix ? "创建可编辑副本" : "安装此版本", onCancel: { market.cancelInstall() }, canCancel: !market.installCommitting) {
                    Task {
                        do {
                            let installed = try await market.install(distribution, approved: true, remix: remix)
                            consenting = false
                            if remix, let authoring = installed.authoring {
                                onDevelop(.init(appId: installed.appId, name: installed.name, sourceDirectory: authoring.sourceDirectory, sourceSessionID: nil, prompt: "继续开发这个应用。"))
                            } else { onOpenApp(installed.appId) }
                        } catch { self.error = error.localizedDescription; consenting = false }
                    }
                }
            }
        }
    }
    private func load() async {
        do { app = try await market.detail(appId); distribution = try await market.distribution(appId); error = nil }
        catch { self.error = error.localizedDescription }
    }
}
