import SwiftUI

// MARK: - Settings page identity (the prototype's `stack` of `{id, ...}`)
enum SettingsPage: Equatable {
    case main, account
    case providerList(ProviderKindBox)
    case providerPicker(ProviderKindBox)
    case providerEdit(ProviderKindBox, String)
    case voice, knowledge, memory, workflows
    case appearance, language, notifications, input, privacy
    case skills, skillDetail(String)
    case mcpList, mcpEdit(String)
    case dream
}

/// Equatable/Hashable wrapper so ProviderKind can live inside the enum.
struct ProviderKindBox: Equatable, Hashable {
    let kind: ProviderKind
    init(_ k: ProviderKind) { kind = k }
    static func == (l: ProviderKindBox, r: ProviderKindBox) -> Bool {
        String(describing: l.kind) == String(describing: r.kind)
    }
    func hash(into h: inout Hasher) { h.combine(String(describing: kind)) }
}

// MARK: - Settings sheet host (slide-up + nav bar + page stack)
struct SettingsHost: View {
    @EnvironmentObject private var app: AppState
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore
    let onClose: () -> Void

    @State private var stack: [SettingsPage] = [.main]

    private var top: SettingsPage { stack.last ?? .main }

    func push(_ page: SettingsPage) { withAnimation(.easeOut(duration: 0.22)) { stack.append(page) } }
    func pop() { if stack.count > 1 { withAnimation(.easeOut(duration: 0.22)) { _ = stack.removeLast() } } }
    func reset() { withAnimation(.easeOut(duration: 0.22)) { stack = [.main] } }
    /// Replaces the top two pages (used by the picker → edit flow).
    func replaceTopTwo(with pages: [SettingsPage]) {
        var s = Array(stack.dropLast(2)); s.append(contentsOf: pages); stack = s
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            Color.black.opacity(0.5).ignoresSafeArea().onTapGesture { onClose() }
            sheet
                .transition(.move(edge: .bottom))
        }
    }

    private var sheet: some View {
        GeometryReader { geo in
            VStack(spacing: 0) {
                Spacer(minLength: 0)
                VStack(spacing: 0) {
                    grabber
                    navBar
                    pageContent
                }
                .frame(height: geo.size.height * 0.94)
                .background(t.windowBg)
                .clipShape(UnevenRoundedRectangle(topLeadingRadius: 20, topTrailingRadius: 20))
                .shadow(color: .black.opacity(0.35), radius: 15, y: -8)
            }
        }
    }

    private var grabber: some View {
        RoundedRectangle(cornerRadius: 99).fill(t.text4.opacity(0.45))
            .frame(width: 38, height: 4.5)
            .padding(.top, 8).padding(.bottom, 4)
    }

    // MARK: nav bar — back (chevron + prev title) / title / Done|x
    private var navBar: some View {
        HStack(spacing: 4) {
            Button {
                stack.count > 1 ? pop() : onClose()
            } label: {
                HStack(spacing: 2) {
                    LXIcon(name: .chevronR, size: 17, color: t.text2, stroke: 2.2)
                        .rotationEffect(.degrees(180))
                    Text(stack.count > 1 ? title(of: stack[stack.count - 2]) : "关闭")
                        .font(.system(size: 14, weight: .medium)).foregroundColor(t.text2).lineLimit(1)
                }
                .frame(maxWidth: 120, alignment: .leading)
            }
            Spacer()
            Text(title(of: top)).font(.system(size: 16, weight: .bold)).foregroundColor(t.text).lineLimit(1)
            Spacer()
            if stack.count > 1 {
                Button { reset() } label: {
                    Text("完成").font(.system(size: 14, weight: .semibold)).foregroundColor(t.accent)
                        .frame(height: 34).padding(.horizontal, 12)
                }
            } else {
                Button(action: onClose) {
                    LXIcon(name: .x, size: 18, color: t.text3, stroke: 1.8).frame(width: 34, height: 34)
                }
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 8).padding(.top, 6).padding(.bottom, 12)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .bottom)
    }

    private var pageContent: some View {
        ScrollView(showsIndicators: false) {
            SettingsPages(store: store, host: self, page: top)
                .padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 28)
        }
        .id(pageKey)
        .transition(.opacity)
    }

    private var pageKey: String { "\(stack.count)-\(title(of: top))" }

    // MARK: titles
    func title(of page: SettingsPage) -> String {
        switch page {
        case .main: return "设置"
        case .account: return "账户"
        case .providerList(let b): return b.kind.title
        case .providerPicker(let b): return "添加 " + (b.kind == .llm ? "LLM" : b.kind == .search ? "搜索" : "抓取")
        case .providerEdit(let b, let id):
            return store.providers(b.kind).first(where: { $0.id == id })?.name ?? "编辑"
        case .voice: return "语音 TTS"
        case .knowledge: return "知识库"
        case .memory: return "记忆"
        case .workflows: return "工作流"
        case .appearance: return "外观"
        case .language: return "语言"
        case .notifications: return "通知"
        case .input: return "键盘与输入"
        case .privacy: return "数据与隐私"
        case .skills: return "Skills"
        case .skillDetail(let id): return store.skills.first(where: { $0.id == id })?.name ?? "Skill"
        case .mcpList: return "MCP 服务器"
        case .mcpEdit(let id): return store.mcpServers.first(where: { $0.id == id })?.name ?? "MCP"
        case .dream: return "Dream 模式"
        }
    }
}
