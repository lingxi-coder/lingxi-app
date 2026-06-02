import SwiftUI

// MARK: - WorkflowBar — horizontal scroll of step chips (done / running / todo)
struct WorkflowBar: View {
    @Environment(\.theme) private var t

    private enum StepState { case done, running, todo }
    private struct Step: Identifiable { let id: Int; let label: String; let state: StepState }
    private let steps: [Step] = [
        .init(id: 1, label: "理解需求", state: .done),
        .init(id: 2, label: "检索 Claude iOS", state: .done),
        .init(id: 3, label: "抽屉/侧栏组件", state: .done),
        .init(id: 4, label: "语音模式集成", state: .running),
        .init(id: 5, label: "导出", state: .todo),
    ]

    @State private var pulse = false

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 5) {
                ForEach(steps) { s in chip(s) }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8).padding(.bottom, 10)
        }
        .background(t.windowBg)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .bottom)
        .onAppear { withAnimation(.easeInOut(duration: 1.4).repeatForever()) { pulse = true } }
    }

    @ViewBuilder private func chip(_ s: Step) -> some View {
        let fg: Color = s.state == .done ? t.ok : (s.state == .running ? t.accent : t.text4)
        let bg: Color = s.state == .done ? t.ok.tint(0.14) : (s.state == .running ? t.accent.tint(0.15) : .clear)
        let stroke: Color = s.state == .done ? t.ok.tint(0.28) : (s.state == .running ? t.accent.tint(0.35) : t.text4.tint(0.30))
        HStack(spacing: 5) {
            switch s.state {
            case .done:    LXIcon(name: .check, size: 10, color: fg, stroke: 2.5)
            case .running: Circle().fill(t.accent).frame(width: 5, height: 5).opacity(pulse ? 0.8 : 0.35)
            case .todo:    Circle().strokeBorder(t.text4, lineWidth: 1).frame(width: 5, height: 5)
            }
            Text(s.label).font(.system(size: 11, weight: .medium))
        }
        .foregroundColor(fg)
        .padding(.horizontal, 9).padding(.vertical, 4)
        .background(bg)
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(stroke, style: StrokeStyle(lineWidth: 0.5, dash: s.state == .todo ? [3,2] : [])))
        .fixedSize()
    }
}
