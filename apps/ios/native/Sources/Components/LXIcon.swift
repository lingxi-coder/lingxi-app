import SwiftUI

enum LXIconName: String, CaseIterable {
    case menu, edit, pencil, search, sparkle, sparkles, book, bookOpen, listFiles, workflow, cog, wrench, plus, mic, paperclip
    case chevron, sun, moon, check, pin, brain, arrowUp, folder, clock, message
    case chevronR, play, pause, x, skill, plug, dream, link, copy, share, listChecks
    case warning, stop, squareStop, arrowRight, audioWave, terminal, globe, appBuilder, mcpFallback
}

extension LXIconName {
    /// One native symbol vocabulary across navigation, settings and conversation.
    var systemName: String {
        switch self {
        case .menu: "sidebar.left"
        case .edit: "square.and.pencil"
        case .pencil: "pencil"
        case .search: "magnifyingglass"
        case .sparkle: "sparkle"
        case .sparkles: "sparkles"
        case .book: "book"
        case .bookOpen: "book.pages"
        case .listFiles: "doc.text"
        case .workflow: "point.3.connected.trianglepath.dotted"
        case .cog: "gearshape"
        case .wrench: "wrench"
        case .plus: "plus"
        case .mic: "mic"
        case .paperclip: "paperclip"
        case .chevron: "chevron.down"
        case .sun: "sun.max"
        case .moon: "moon"
        case .check: "checkmark"
        case .pin: "pin"
        case .brain: "brain"
        case .arrowUp: "arrow.up"
        case .folder: "folder"
        case .clock: "clock"
        case .message: "bubble.left"
        case .chevronR: "chevron.forward"
        case .play: "play.fill"
        case .pause: "pause.fill"
        case .x: "xmark"
        case .skill: "bolt"
        case .plug: "powerplug"
        case .dream: "moon.stars"
        case .link: "link"
        case .copy: "doc.on.doc"
        case .share: "square.and.arrow.up"
        case .listChecks: "checklist"
        case .warning: "exclamationmark.triangle"
        case .stop: "stop.fill"
        case .squareStop: "stop.circle"
        case .arrowRight: "arrow.forward"
        case .audioWave: "waveform"
        case .terminal: "terminal"
        case .globe: "globe"
        case .appBuilder: "app.badge"
        case .mcpFallback: "circle"
        }
    }
}

struct LXIcon: View {
    let name: LXIconName
    var size: CGFloat = 20
    var color: Color = .primary
    var stroke: CGFloat = 1.7

    var body: some View {
        Group {
            if name == .mcpFallback {
                Canvas { context, canvas in
                    let scale = canvas.width / 16
                    var path = Path()
                    for x in [CGFloat(4.5), CGFloat(11.5)] {
                        for y in [CGFloat(4.5), CGFloat(11.5)] {
                            path.addEllipse(in: CGRect(x: (x - 2.25) * scale, y: (y - 2.25) * scale, width: 4.5 * scale, height: 4.5 * scale))
                        }
                    }
                    path.move(to: CGPoint(x: 6.1 * scale, y: 9.9 * scale))
                    path.addLine(to: CGPoint(x: 9.9 * scale, y: 6.1 * scale))
                    context.stroke(path, with: .color(color), lineWidth: 1.05 * scale)
                }
            } else {
                Image(systemName: name.systemName)
                    .font(.system(size: size, weight: stroke >= 2 ? .semibold : .regular))
                    .foregroundStyle(color)
            }
        }.frame(width: size, height: size)
    }
}
