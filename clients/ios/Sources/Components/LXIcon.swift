import SwiftUI

// MARK: - LXIcon
//
// Recreates the prototype's inline-SVG `<Icon name=…>` set. Each case draws the
// same path data from the HTML (24×24 viewBox), scaled to `size`. Stroke icons
// use round caps/joins matching the original `strokeLinecap/Linejoin="round"`.

enum LXIconName: String {
    case menu, edit, search, sparkle, book, workflow, cog, plus, mic, paperclip
    case chevron, sun, moon, check, pin, brain, arrowUp, folder, clock, message
    case chevronR, play, pause, x, skill, plug, dream, link, copy, share
    case warning, stop, arrowRight
}

struct LXIcon: View {
    let name: LXIconName
    var size: CGFloat = 20
    var color: Color = .primary
    var stroke: CGFloat = 1.7

    var body: some View {
        Canvas { ctx, sz in
            let s = sz.width / 24.0           // scale from 24-unit viewBox
            let stroked = StrokeStyle(lineWidth: stroke * s, lineCap: .round, lineJoin: .round)
            let shading = GraphicsContext.Shading.color(color)
            let scale = CGAffineTransform(scaleX: s, y: s)

            for spec in LXIcon.strokePaths(name) {
                ctx.stroke(spec.path().applying(scale), with: shading, style: stroked)
            }
            for spec in LXIcon.fillPaths(name) {
                ctx.fill(spec.path().applying(scale), with: shading)
            }
        }
        .frame(width: size, height: size)
    }
}

// MARK: - Path data
//
// Helpers to build Paths from the SVG primitives used in the prototype.
private struct PathSpec { let path: () -> Path }

private func line(_ pts: [(CGFloat, CGFloat)]) -> Path {
    var p = Path()
    guard let first = pts.first else { return p }
    p.move(to: CGPoint(x: first.0, y: first.1))
    for pt in pts.dropFirst() { p.addLine(to: CGPoint(x: pt.0, y: pt.1)) }
    return p
}
private func circle(_ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat) -> Path {
    Path(ellipseIn: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
}
private func roundedRect(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) -> Path {
    Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: r)
}

extension LXIcon {
    fileprivate static func strokePaths(_ name: LXIconName) -> [PathSpec] {
        switch name {
        case .menu:
            return [PathSpec { line([(3,6),(21,6)]) }, PathSpec { line([(3,12),(21,12)]) }, PathSpec { line([(3,18),(21,18)]) }]
        case .edit:
            return [PathSpec { line([(12,20),(21,20)]) },
                    PathSpec { var p = Path(); p.move(to: .init(x:16.5,y:3.5)); p.addCurve(to: .init(x:19.5,y:6.5), control1: .init(x:17.66,y:3.5), control2: .init(x:19.5,y:5.34)); p.addLine(to: .init(x:7,y:19)); p.addLine(to: .init(x:3,y:20)); p.addLine(to: .init(x:4,y:16)); p.closeSubpath(); return p }]
        case .search:
            return [PathSpec { circle(11,11,7) }, PathSpec { line([(21,21),(16.7,16.7)]) }]
        case .sparkle:
            return [PathSpec { line([(12,3),(12,6)]) }, PathSpec { line([(12,18),(12,21)]) },
                    PathSpec { line([(3,12),(6,12)]) }, PathSpec { line([(18,12),(21,12)]) },
                    PathSpec { line([(5.6,5.6),(7.7,7.7)]) }, PathSpec { line([(16.3,16.3),(18.4,18.4)]) },
                    PathSpec { line([(5.6,18.4),(7.7,16.3)]) }, PathSpec { line([(16.3,7.7),(18.4,5.6)]) },
                    PathSpec { circle(12,12,2.5) }]
        case .book:
            return [PathSpec { var p = Path(); p.move(to: .init(x:4,y:19.5)); p.addCurve(to: .init(x:6.5,y:17), control1: .init(x:4,y:18.12), control2: .init(x:5.12,y:17)); p.addLine(to: .init(x:20,y:17)); return p },
                    PathSpec { var p = Path(); p.move(to: .init(x:6.5,y:2)); p.addLine(to: .init(x:20,y:2)); p.addLine(to: .init(x:20,y:22)); p.addLine(to: .init(x:6.5,y:22)); p.addCurve(to: .init(x:4,y:19.5), control1: .init(x:5.12,y:22), control2: .init(x:4,y:20.88)); p.addLine(to: .init(x:4,y:4.5)); p.addCurve(to: .init(x:6.5,y:2), control1: .init(x:4,y:3.12), control2: .init(x:5.12,y:2)); p.closeSubpath(); return p }]
        case .workflow:
            return [PathSpec { roundedRect(3,3,6,6,1.5) }, PathSpec { roundedRect(15,3,6,6,1.5) }, PathSpec { roundedRect(9,15,6,6,1.5) },
                    PathSpec { var p = Path(); p.move(to: .init(x:6,y:9)); p.addLine(to: .init(x:6,y:11)); p.addCurve(to: .init(x:8,y:13), control1: .init(x:6,y:12.1), control2: .init(x:6.9,y:13)); p.addLine(to: .init(x:16,y:13)); p.addCurve(to: .init(x:18,y:11), control1: .init(x:17.1,y:13), control2: .init(x:18,y:12.1)); p.addLine(to: .init(x:18,y:9)); return p }]
        case .cog:
            return [PathSpec { circle(12,12,3) },
                    PathSpec { var p = Path(); p.move(to: .init(x:19.4,y:15)); p.addCurve(to: .init(x:19.73,y:16.82), control1: .init(x:19.18,y:15.66), control2: .init(x:19.3,y:16.39)); p.addLine(to: .init(x:19.79,y:16.88)); p.addCurve(to: .init(x:16.96,y:19.71), control1: .init(x:20.57,y:17.66), control2: .init(x:17.74,y:20.49)); p.addLine(to: .init(x:16.9,y:19.65)); p.addCurve(to: .init(x:15.08,y:19.32), control1: .init(x:16.61,y:19.36), control2: .init(x:15.74,y:19.1)); p.addCurve(to: .init(x:14.08,y:20.83), control1: .init(x:14.42,y:19.54), control2: .init(x:14.08,y:20.13)); p.addLine(to: .init(x:14.08,y:21)); p.addCurve(to: .init(x:10.08,y:21), control1: .init(x:14.08,y:22.1), control2: .init(x:10.08,y:22.1)); p.addLine(to: .init(x:10.08,y:20.91)); p.addCurve(to: .init(x:9.08,y:19.4), control1: .init(x:10.08,y:20.17), control2: .init(x:9.74,y:19.62)); p.addCurve(to: .init(x:7.26,y:19.73), control1: .init(x:8.42,y:19.18), control2: .init(x:7.69,y:19.3)); p.addLine(to: .init(x:7.2,y:19.79)); p.addCurve(to: .init(x:4.37,y:16.96), control1: .init(x:6.42,y:20.57), control2: .init(x:3.59,y:17.74)); p.addLine(to: .init(x:4.43,y:16.9)); p.addCurve(to: .init(x:4.76,y:15.08), control1: .init(x:4.72,y:16.61), control2: .init(x:4.98,y:15.74)); p.addCurve(to: .init(x:3.25,y:14.08), control1: .init(x:4.54,y:14.42), control2: .init(x:3.95,y:14.08)); p.addLine(to: .init(x:3,y:14.08)); p.addCurve(to: .init(x:3,y:10.08), control1: .init(x:1.9,y:14.08), control2: .init(x:1.9,y:10.08)); p.addLine(to: .init(x:3.09,y:10.08)); p.addCurve(to: .init(x:4.6,y:9.08), control1: .init(x:3.83,y:10.08), control2: .init(x:4.38,y:9.74)); p.addCurve(to: .init(x:4.27,y:7.26), control1: .init(x:4.82,y:8.42), control2: .init(x:4.7,y:7.69)); p.addLine(to: .init(x:4.21,y:7.2)); p.addCurve(to: .init(x:7.04,y:4.37), control1: .init(x:3.43,y:6.42), control2: .init(x:6.26,y:3.59)); p.addLine(to: .init(x:7.1,y:4.43)); p.addCurve(to: .init(x:8.92,y:4.76), control1: .init(x:7.39,y:4.72), control2: .init(x:8.26,y:4.98)); p.addCurve(to: .init(x:9.92,y:3.25), control1: .init(x:9.58,y:4.54), control2: .init(x:9.92,y:3.95)); p.addLine(to: .init(x:9.92,y:3)); p.addCurve(to: .init(x:13.92,y:3), control1: .init(x:9.92,y:1.9), control2: .init(x:13.92,y:1.9)); p.addLine(to: .init(x:13.92,y:3.09)); p.addCurve(to: .init(x:14.92,y:4.6), control1: .init(x:13.92,y:3.83), control2: .init(x:14.26,y:4.38)); p.addCurve(to: .init(x:16.74,y:4.27), control1: .init(x:15.58,y:4.82), control2: .init(x:16.31,y:4.7)); p.addLine(to: .init(x:16.8,y:4.21)); p.addCurve(to: .init(x:19.63,y:7.04), control1: .init(x:17.58,y:3.43), control2: .init(x:20.41,y:6.26)); p.addLine(to: .init(x:19.57,y:7.1)); p.addCurve(to: .init(x:19.24,y:8.92), control1: .init(x:19.28,y:7.39), control2: .init(x:19.02,y:8.26)); p.addLine(to: .init(x:19.24,y:9)); p.addCurve(to: .init(x:20.75,y:10), control1: .init(x:19.46,y:9.66), control2: .init(x:20.05,y:10)); p.addLine(to: .init(x:21,y:10)); p.addCurve(to: .init(x:21,y:14), control1: .init(x:22.1,y:10), control2: .init(x:22.1,y:14)); p.addLine(to: .init(x:20.91,y:14)); p.addCurve(to: .init(x:19.4,y:15), control1: .init(x:20.17,y:14), control2: .init(x:19.62,y:14.34)); p.closeSubpath(); return p }]
        case .plus:
            return [PathSpec { line([(12,5),(12,19)]) }, PathSpec { line([(5,12),(19,12)]) }]
        case .mic:
            return [PathSpec { roundedRect(9,2,6,11,3) },
                    PathSpec { var p = Path(); p.move(to: .init(x:19,y:10)); p.addLine(to: .init(x:19,y:12)); p.addCurve(to: .init(x:5,y:12), control1: .init(x:19,y:15.87), control2: .init(x:5,y:15.87)); p.addLine(to: .init(x:5,y:10)); return p },
                    PathSpec { line([(12,19),(12,22)]) }]
        case .paperclip:
            return [PathSpec { var p = Path(); p.move(to: .init(x:21.44,y:11.05)); p.addLine(to: .init(x:12.25,y:20.24)); p.addCurve(to: .init(x:3.76,y:11.75), control1: .init(x:9.9,y:22.59), control2: .init(x:6.11,y:22.59)); p.addLine(to: .init(x:12.33,y:3.18)); p.addCurve(to: .init(x:17.93,y:8.83), control1: .init(x:13.9,y:1.61), control2: .init(x:19.5,y:7.26)); p.addLine(to: .init(x:9.34,y:17.4)); p.addCurve(to: .init(x:6.51,y:14.57), control1: .init(x:8.56,y:18.18), control2: .init(x:5.73,y:15.35)); p.addLine(to: .init(x:15,y:6.09)); return p }]
        case .chevron:
            return [PathSpec { line([(6,9),(12,15),(18,9)]) }]
        case .chevronR:
            return [PathSpec { line([(9,18),(15,12),(9,6)]) }]
        case .arrowRight:
            return [PathSpec { line([(5,12),(19,12)]) }, PathSpec { line([(12,5),(19,12),(12,19)]) }]
        case .x:
            return [PathSpec { line([(18,6),(6,18)]) }, PathSpec { line([(6,6),(18,18)]) }]
        case .sun:
            return [PathSpec { circle(12,12,4) },
                    PathSpec { line([(12,2),(12,4)]) }, PathSpec { line([(12,20),(12,22)]) },
                    PathSpec { line([(4.93,4.93),(6.34,6.34)]) }, PathSpec { line([(17.66,17.66),(19.07,19.07)]) },
                    PathSpec { line([(2,12),(4,12)]) }, PathSpec { line([(20,12),(22,12)]) },
                    PathSpec { line([(6.34,17.66),(4.93,19.07)]) }, PathSpec { line([(19.07,4.93),(17.66,6.34)]) }]
        case .moon, .dream:
            return [PathSpec { var p = Path(); p.move(to: .init(x:21,y:12.79)); p.addCurve(to: .init(x:11.21,y:3), control1: .init(x:20.27,y:18.2), control2: .init(x:15.27,y:3.73)); p.addCurve(to: .init(x:21,y:12.79), control1: .init(x:14.73,y:3.73), control2: .init(x:20.27,y:9.6)); p.closeSubpath(); return p }]
        case .check:
            return [PathSpec { line([(20,6),(9,17),(4,12)]) }]
        case .pin:
            return [PathSpec { line([(12,17),(12,22)]) },
                    PathSpec { var p = Path(); p.move(to: .init(x:9,y:10.76)); p.addLine(to: .init(x:9,y:3)); p.addLine(to: .init(x:15,y:3)); p.addLine(to: .init(x:15,y:10.76)); p.addLine(to: .init(x:18,y:12.7)); p.addLine(to: .init(x:18,y:16)); p.addLine(to: .init(x:6,y:16)); p.addLine(to: .init(x:6,y:12.7)); p.closeSubpath(); return p }]
        case .brain:
            return [PathSpec { var p = Path(); p.move(to: .init(x:9.5,y:2)); p.addCurve(to: .init(x:12,y:4.5), control1: .init(x:10.88,y:2), control2: .init(x:12,y:3.12)); p.addLine(to: .init(x:12,y:19.5)); p.addCurve(to: .init(x:7.04,y:19.94), control1: .init(x:12,y:20.88), control2: .init(x:8.5,y:21.5)); p.addCurve(to: .init(x:4.5,y:17), control1: .init(x:5.5,y:19.5), control2: .init(x:4.5,y:18.4)); p.addCurve(to: .init(x:2.5,y:13.35), control1: .init(x:3.4,y:16.6), control2: .init(x:2.5,y:14.6)); p.addCurve(to: .init(x:4,y:8.5), control1: .init(x:2.8,y:11.2), control2: .init(x:3,y:9.3)); p.addCurve(to: .init(x:6.5,y:6), control1: .init(x:4,y:7.12), control2: .init(x:5.12,y:6)); p.addCurve(to: .init(x:9.5,y:2), control1: .init(x:6.5,y:3.79), control2: .init(x:7.5,y:2)); p.closeSubpath(); return p },
                    PathSpec { var p = Path(); p.move(to: .init(x:14.5,y:2)); p.addCurve(to: .init(x:12,y:4.5), control1: .init(x:13.12,y:2), control2: .init(x:12,y:3.12)); p.addLine(to: .init(x:12,y:19.5)); p.addCurve(to: .init(x:16.96,y:19.94), control1: .init(x:12,y:20.88), control2: .init(x:15.5,y:21.5)); p.addCurve(to: .init(x:19.5,y:17), control1: .init(x:18.5,y:19.5), control2: .init(x:19.5,y:18.4)); p.addCurve(to: .init(x:21.5,y:13.35), control1: .init(x:20.6,y:16.6), control2: .init(x:21.5,y:14.6)); p.addCurve(to: .init(x:20,y:8.5), control1: .init(x:21.2,y:11.2), control2: .init(x:21,y:9.3)); p.addCurve(to: .init(x:17.5,y:6), control1: .init(x:20,y:7.12), control2: .init(x:18.88,y:6)); p.addCurve(to: .init(x:14.5,y:2), control1: .init(x:17.5,y:3.79), control2: .init(x:16.5,y:2)); p.closeSubpath(); return p }]
        case .folder:
            return [PathSpec { var p = Path(); p.move(to: .init(x:3,y:7)); p.addCurve(to: .init(x:5,y:5), control1: .init(x:3,y:5.9), control2: .init(x:3.9,y:5)); p.addLine(to: .init(x:9,y:5)); p.addLine(to: .init(x:11,y:7)); p.addLine(to: .init(x:19,y:7)); p.addCurve(to: .init(x:21,y:9), control1: .init(x:20.1,y:7), control2: .init(x:21,y:7.9)); p.addLine(to: .init(x:21,y:18)); p.addCurve(to: .init(x:19,y:20), control1: .init(x:21,y:19.1), control2: .init(x:20.1,y:20)); p.addLine(to: .init(x:5,y:20)); p.addCurve(to: .init(x:3,y:18), control1: .init(x:3.9,y:20), control2: .init(x:3,y:19.1)); p.closeSubpath(); return p }]
        case .clock:
            return [PathSpec { circle(12,12,9) }, PathSpec { line([(12,7),(12,12),(15,14)]) }]
        case .message:
            return [PathSpec { var p = Path(); p.move(to: .init(x:21,y:11.5)); p.addCurve(to: .init(x:20.1,y:15.3), control1: .init(x:21,y:12.83), control2: .init(x:20.69,y:14.13)); p.addCurve(to: .init(x:12.5,y:20), control1: .init(x:18.65,y:18.18), control2: .init(x:15.7,y:20)); p.addCurve(to: .init(x:8.7,y:19.1), control1: .init(x:11.17,y:20), control2: .init(x:9.87,y:19.69)); p.addLine(to: .init(x:3,y:21)); p.addLine(to: .init(x:4.9,y:15.3)); p.addCurve(to: .init(x:4,y:11.5), control1: .init(x:4.31,y:14.13), control2: .init(x:4,y:12.83)); p.addCurve(to: .init(x:8.7,y:3.9), control1: .init(x:4,y:8.3), control2: .init(x:5.82,y:5.35)); p.addCurve(to: .init(x:12.5,y:3), control1: .init(x:9.87,y:3.31), control2: .init(x:11.17,y:3)); p.addLine(to: .init(x:13,y:3)); p.addCurve(to: .init(x:21,y:11), control1: .init(x:17.39,y:3.25), control2: .init(x:20.75,y:6.61)); p.closeSubpath(); return p }]
        case .skill:
            return [PathSpec { circle(12,8,3.5) },
                    PathSpec { var p = Path(); p.move(to: .init(x:5,y:21)); p.addLine(to: .init(x:5,y:19)); p.addCurve(to: .init(x:9,y:15), control1: .init(x:5,y:16.79), control2: .init(x:6.79,y:15)); p.addLine(to: .init(x:15,y:15)); p.addCurve(to: .init(x:19,y:19), control1: .init(x:17.21,y:15), control2: .init(x:19,y:16.79)); p.addLine(to: .init(x:19,y:21)); return p },
                    PathSpec { line([(19,4),(20,5),(19,6)]) }, PathSpec { line([(5,4),(4,5),(5,6)]) }]
        case .plug:
            return [PathSpec { line([(9,2),(9,8)]) }, PathSpec { line([(15,2),(15,8)]) },
                    PathSpec { var p = Path(); p.move(to: .init(x:6,y:8)); p.addLine(to: .init(x:18,y:8)); p.addLine(to: .init(x:18,y:11)); p.addCurve(to: .init(x:6,y:11), control1: .init(x:18,y:14.31), control2: .init(x:6,y:14.31)); p.closeSubpath(); return p },
                    PathSpec { line([(12,17),(12,22)]) }]
        case .link:
            return [PathSpec { var p = Path(); p.move(to: .init(x:10,y:13)); p.addCurve(to: .init(x:13.5,y:14.5), control1: .init(x:10.79,y:14.06), control2: .init(x:12.27,y:14.79)); p.addLine(to: .init(x:18,y:10)); p.addCurve(to: .init(x:14,y:6), control1: .init(x:20.21,y:7.79), control2: .init(x:16.21,y:3.79)); p.addLine(to: .init(x:11.5,y:8.5)); return p },
                    PathSpec { var p = Path(); p.move(to: .init(x:14,y:11)); p.addCurve(to: .init(x:10.5,y:9.5), control1: .init(x:13.21,y:9.94), control2: .init(x:11.73,y:9.21)); p.addLine(to: .init(x:6,y:14)); p.addCurve(to: .init(x:10,y:18), control1: .init(x:3.79,y:16.21), control2: .init(x:7.79,y:20.21)); p.addLine(to: .init(x:12.5,y:15.5)); return p }]
        case .copy:
            return [PathSpec { roundedRect(9,9,13,13,2) },
                    PathSpec { var p = Path(); p.move(to: .init(x:5,y:15)); p.addLine(to: .init(x:4,y:15)); p.addCurve(to: .init(x:2,y:13), control1: .init(x:2.9,y:15), control2: .init(x:2,y:14.1)); p.addLine(to: .init(x:2,y:4)); p.addCurve(to: .init(x:4,y:2), control1: .init(x:2,y:2.9), control2: .init(x:2.9,y:2)); p.addLine(to: .init(x:13,y:2)); p.addCurve(to: .init(x:15,y:4), control1: .init(x:14.1,y:2), control2: .init(x:15,y:2.9)); p.addLine(to: .init(x:15,y:5)); return p }]
        case .share:
            // Three nodes connected by two edges (the classic share glyph),
            // mirroring Android LXIconName.Share.
            return [PathSpec { circle(18, 5, 3) },
                    PathSpec { circle(6, 12, 3) },
                    PathSpec { circle(18, 19, 3) },
                    PathSpec { line([(8.59, 13.51), (15.42, 17.49)]) },
                    PathSpec { line([(15.41, 6.51), (8.59, 10.49)]) }]
        case .warning:
            // Triangle-exclamation: a rounded warning triangle + the bang stem;
            // the bang dot is drawn as a fill (see fillPaths).
            return [PathSpec { var p = Path(); p.move(to: .init(x:10.29,y:3.86)); p.addLine(to: .init(x:1.82,y:18)); p.addCurve(to: .init(x:3.53,y:21), control1: .init(x:1.45,y:18.64), control2: .init(x:2.78,y:21)); p.addLine(to: .init(x:20.47,y:21)); p.addCurve(to: .init(x:22.18,y:18), control1: .init(x:21.22,y:21), control2: .init(x:22.55,y:18.64)); p.addLine(to: .init(x:13.71,y:3.86)); p.addCurve(to: .init(x:10.29,y:3.86), control1: .init(x:12.93,y:2.6), control2: .init(x:11.07,y:2.6)); p.closeSubpath(); return p },
                    PathSpec { line([(12,9),(12,13)]) }]
        case .arrowUp, .play, .pause, .stop:
            return [] // filled icons
        }
    }

    fileprivate static func fillPaths(_ name: LXIconName) -> [PathSpec] {
        switch name {
        case .arrowUp:
            return [PathSpec { line([(12,4),(5,12),(9,12),(9,20),(15,20),(15,12),(19,12)]) }]
        case .play:
            return [PathSpec { line([(6,4),(20,12),(6,20)]) }]
        case .pause:
            return [PathSpec { roundedRect(6,5,4,14,1) }, PathSpec { roundedRect(14,5,4,14,1) }]
        case .stop:
            // A filled rounded square — the universal "stop the stream" glyph.
            return [PathSpec { roundedRect(6,6,12,12,2.5) }]
        case .warning:
            // The exclamation dot at the base of the bang.
            return [PathSpec { circle(12,17,1.05) }]
        default:
            return []
        }
    }
}
