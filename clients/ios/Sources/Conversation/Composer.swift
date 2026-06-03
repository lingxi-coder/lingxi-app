import SwiftUI

// MARK: - Composer (pill text field + model chip + attach + send/mic)
struct Composer: View {
    @Environment(\.theme) private var t
    @Binding var model: ModelOption
    let onSend: (String) -> Void

    @State private var text = ""
    @State private var modelOpen = false

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                TextField("", text: $text, prompt: Text("向灵犀提问…").foregroundColor(t.text4), axis: .vertical)
                    .font(.system(size: 15.5))
                    .foregroundColor(t.text)
                    .lineLimit(1...5)
                    .padding(.horizontal, 4).padding(.vertical, 2)

                HStack(spacing: 2) {
                    Button(action: {}) {
                        LXIcon(name: .plus, size: 18, color: t.text3, stroke: 1.8)
                            .frame(width: 34, height: 34)
                    }
                    modelChip
                    Spacer()
                    if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Button(action: {}) {
                            LXIcon(name: .mic, size: 18, color: t.text2, stroke: 1.8)
                                .frame(width: 34, height: 34)
                        }
                    } else {
                        Button(action: send) {
                            LXIcon(name: .arrowUp, size: 16, color: .white)
                                .frame(width: 34, height: 34)
                                .background(t.accent)
                                .clipShape(RoundedRectangle(cornerRadius: 10))
                                .shadow(color: t.accent.tint(0.40), radius: 6, y: 4)
                        }
                    }
                }
            }
            .padding(.horizontal, 12).padding(.top, 10).padding(.bottom, 8)
            .background(t.composerBg)
            .clipShape(RoundedRectangle(cornerRadius: 22))
            .overlay(RoundedRectangle(cornerRadius: 22).stroke(t.borderStrong, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.06), radius: 8, y: 4)
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 4)
        .overlay(alignment: .bottomLeading) {
            if modelOpen { modelMenu.padding(.leading, 50).padding(.bottom, 50) }
        }
    }

    private var modelChip: some View {
        Button { withAnimation(.easeOut(duration: 0.15)) { modelOpen.toggle() } } label: {
            HStack(spacing: 5) {
                Circle().fill(model.color).frame(width: 6, height: 6)
                Text(model.shortName).font(.system(size: 12, weight: .medium))
                LXIcon(name: .chevron, size: 11, color: t.text4, stroke: 2)
            }
            .foregroundColor(t.text2)
            .padding(.horizontal, 9).padding(.vertical, 5)
            .background(modelOpen ? t.surfaceHover : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
    }

    private var modelMenu: some View {
        VStack(spacing: 0) {
            ForEach(MockData.models) { m in
                Button {
                    model = m; withAnimation(.easeOut(duration: 0.15)) { modelOpen = false }
                } label: {
                    HStack(spacing: 10) {
                        Circle().fill(m.color).frame(width: 8, height: 8)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(m.name).font(.system(size: 13, weight: .medium)).foregroundColor(t.text)
                            Text(m.desc).font(.system(size: 11)).foregroundColor(t.text3)
                        }
                        Spacer()
                    }
                    .padding(.horizontal, 10).padding(.vertical, 8)
                    .background(m.id == model.id ? t.accent.tint(0.15) : .clear)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                }
            }
        }
        .padding(5)
        .frame(width: 200)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.borderStrong, lineWidth: 0.5))
        .shadow(color: .black.opacity(0.3), radius: 20, y: 16)
        .transition(.opacity.combined(with: .scale(scale: 0.95, anchor: .bottomLeading)))
    }

    private func send() {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        onSend(text); text = ""
    }
}
