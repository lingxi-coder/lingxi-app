import SwiftUI
import UIKit

struct PlanDocument: Equatable {
    let markdown: String
    let isWriting: Bool

    static func tool(_ name: String, json: String) -> Self? {
        guard name.lowercased().replacingOccurrences(of: "_", with: "") == "exitplanmode",
              let data = json.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let plan = object["plan"] as? String, !plan.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return Self(markdown: plan, isWriting: false)
    }

    enum Segment: Equatable {
        case text(String)
        case plan(PlanDocument)
    }

    static func segments(_ text: String) -> [Segment] {
        var result: [Segment] = []
        let remainder = text[...]
        // Only protocol tags on their own lines are documents. Inline examples
        // and tags inside fenced code remain ordinary Markdown.
        var fence: (marker: Character, length: Int)?
        var before = ""
        var plan: String?
        for line in remainder.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            let indentation = line.prefix { $0 == " " }.count
            if indentation <= 3, let marker = trimmed.first, marker == "`" || marker == "~" {
                let length = trimmed.prefix { $0 == marker }.count
                let suffix = trimmed.dropFirst(length)
                if let current = fence {
                    if marker == current.marker, length >= current.length,
                       suffix.trimmingCharacters(in: .whitespaces).isEmpty {
                        fence = nil
                    }
                } else if length >= 3, marker != "`" || !suffix.contains("`") {
                    fence = (marker, length)
                }
            }
            if fence == nil && trimmed == "<proposed_plan>" && plan == nil {
                if !before.isEmpty { result.append(.text(before)); before = "" }
                plan = ""
            } else if fence == nil && trimmed == "</proposed_plan>", let content = plan {
                result.append(.plan(Self(markdown: content.trimmingCharacters(in: .whitespacesAndNewlines), isWriting: false)))
                plan = nil
            } else if plan != nil {
                plan! += String(line) + "\n"
            } else {
                before += String(line) + "\n"
            }
        }
        if let plan { result.append(.plan(Self(markdown: plan.trimmingCharacters(in: .whitespacesAndNewlines), isWriting: true))) }
        if !before.isEmpty { result.append(.text(before.trimmingCharacters(in: .newlines))) }
        return result
    }
}

struct PlanAwareText: View {
    let markdown: String
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(PlanDocument.segments(markdown).enumerated()), id: \.offset) { _, segment in
                switch segment {
                case let .text(text): AIText(markdown: text).equatable()
                case let .plan(plan): PlanDocumentCard(document: plan)
                }
            }
        }
    }
}

private struct OpenPlanDocumentKey: EnvironmentKey {
    static let defaultValue: ((PlanDocument, PlanDocument?) -> Bool)? = nil
}

extension EnvironmentValues {
    var openPlanDocument: ((PlanDocument, PlanDocument?) -> Bool)? {
        get { self[OpenPlanDocumentKey.self] }
        set { self[OpenPlanDocumentKey.self] = newValue }
    }
}

struct PlanDocumentCard: View {
    @Environment(\.theme) private var theme
    @Environment(\.openPlanDocument) private var openPlanDocument
    @State private var isPresented = false
    let document: PlanDocument
    var body: some View {
        Button { if openPlanDocument?(document, nil) != true { isPresented = true } } label: {
            VStack(alignment: .leading, spacing: 18) {
                Label(document.isWriting ? String(localized: "chat_plan_document_writing") : String(localized: "chat_plan_document_title"), systemImage: "lightbulb")
                    .font(.subheadline).foregroundStyle(theme.text3)
                AIText(markdown: document.markdown)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxHeight: 230, alignment: .top).clipped()
                    .mask(alignment: .bottom) {
                        VStack(spacing: 0) {
                            Rectangle()
                            LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom).frame(height: 36)
                        }
                    }
                    .allowsHitTesting(false)
            }
            .padding(18).frame(maxWidth: .infinity, alignment: .leading)
            .background(theme.surface, in: .rect(cornerRadius: 16))
            .overlay(RoundedRectangle(cornerRadius: 16).stroke(theme.border, lineWidth: 1))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("chat.plan-document.preview")
        .accessibilityHint("plan_document_open")
        .onChange(of: document) { old, new in _ = openPlanDocument?(new, old) }
        .sheet(isPresented: $isPresented) {
            NavigationStack {
                PlanDocumentDetail(document: document)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("chat_run_collapse") { isPresented = false }
                        }
                    }
            }
            .environment(\.theme, theme)
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
            .accessibilityIdentifier("conversation.plan-detail-sheet")
        }
    }
}

struct PlanDocumentDetail: View {
    @Environment(\.theme) private var theme
    let document: PlanDocument
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Label("chat_plan_document_title", systemImage: "lightbulb")
                    .font(.subheadline).foregroundStyle(theme.text3)
                Spacer()
                Button { UIPasteboard.general.string = document.markdown } label: {
                    Label("plan_document_copy", systemImage: "doc.on.doc")
                        .labelStyle(.iconOnly)
                        .frame(minWidth: 44, minHeight: 44)
                }
                .accessibilityIdentifier("chat.plan-document.copy")
            }
            .padding(.horizontal, 24)
            ScrollView {
                AIText(markdown: document.markdown)
                    .textSelection(.enabled)
                    .frame(maxWidth: 850, alignment: .leading)
                    .padding(24).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .background(theme.windowBg)
        // `.accessibilityIdentifier` on a container REPLACES every descendant's
        // identifier unless the container is declared a containing element, so
        // without this the copy button's `chat.plan-document.copy` does not
        // exist at runtime even though it is right there in the source.
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("chat.plan-document.detail")
    }
}
