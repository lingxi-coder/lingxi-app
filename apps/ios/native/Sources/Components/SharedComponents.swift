import SwiftUI

// MARK: - Dynamic Type helper
//
// The prototype's layout is pixel-matched with fixed point sizes
// (`.font(.system(size:))`), which do NOT scale with the user's text-size
// setting. For the primary readable content we want the design's baseline size
// AND Dynamic Type scaling. `Font.system(size:weight:)` has no `relativeTo:`
// overload, and the empty-named `Font.custom("", size:relativeTo:)` idiom logs
// a runtime "Unable to update Font Descriptor's weight" warning the moment a
// weight is applied. So instead we scale the baseline point size through
// `UIFontMetrics(forTextStyle:)` — which honors the current Dynamic Type
// content-size category — and build a proper SYSTEM font of that scaled size +
// weight (no malformed descriptor, no warning).
extension Font {
    /// A system font at `size`, scaling with Dynamic Type relative to `style`.
    /// The baseline point size is run through `UIFontMetrics(forTextStyle:)` so
    /// it tracks the user's text-size setting while preserving the design size.
    static func scaledSystem(_ size: CGFloat,
                             weight: Font.Weight = .regular,
                             relativeTo style: Font.TextStyle = .body) -> Font {
        #if canImport(UIKit)
            let scaled = UIFontMetrics(forTextStyle: style.uiTextStyle).scaledValue(for: size)
            return .system(size: scaled, weight: weight)
        #else
            return .system(size: size, weight: weight)
        #endif
    }
}

#if canImport(UIKit)
    private extension Font.TextStyle {
        /// Map the SwiftUI text style to its UIKit counterpart for `UIFontMetrics`.
        var uiTextStyle: UIFont.TextStyle {
            switch self {
            case .largeTitle:  return .largeTitle
            case .title:       return .title1
            case .title2:      return .title2
            case .title3:      return .title3
            case .headline:    return .headline
            case .subheadline: return .subheadline
            case .body:        return .body
            case .callout:     return .callout
            case .footnote:    return .footnote
            case .caption:     return .caption1
            case .caption2:    return .caption2
            @unknown default:  return .body
            }
        }
    }
#endif

// MARK: - color-mix helper
//
// The prototype leans on CSS `color-mix(in oklab, A p%, B)`. We approximate it
// with a straight RGB interpolation (visually indistinguishable for the small
// tints used here). `mix(a, b, p)` == p% of `a` blended onto `b`.
extension Color {
    func mix(with other: Color, amount: Double) -> Color {
        let a = resolveRGBA(); let b = other.resolveRGBA()
        let p = max(0, min(1, amount))
        return Color(.sRGB,
                     red:   a.r * p + b.r * (1 - p),
                     green: a.g * p + b.g * (1 - p),
                     blue:  a.b * p + b.b * (1 - p),
                     opacity: a.a * p + b.a * (1 - p))
    }

    /// `color-mix(in oklab, self p%, transparent)` — i.e. self at p% opacity.
    func tint(_ amount: Double) -> Color {
        let c = resolveRGBA()
        return Color(.sRGB, red: c.r, green: c.g, blue: c.b, opacity: c.a * max(0, min(1, amount)))
    }

    fileprivate func resolveRGBA() -> (r: Double, g: Double, b: Double, a: Double) {
        #if canImport(UIKit)
        let ui = UIColor(self)
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        ui.getRed(&r, green: &g, blue: &b, alpha: &a)
        return (Double(r), Double(g), Double(b), Double(a))
        #else
        return (0, 0, 0, 1)
        #endif
    }
}

// MARK: - Pill (chip)
struct Pill: View {
    @Environment(\.theme) private var t
    let text: String
    var color: Color? = nil

    var body: some View {
        let fg = color ?? t.text3
        Text(text)
            .font(.system(size: 11.5, weight: .medium))
            .padding(.horizontal, 9).padding(.vertical, 3)
            .foregroundColor(fg)
            .background(color != nil ? color!.tint(0.16) : t.surface)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6)
                .stroke(color != nil ? color!.tint(0.30) : t.border, lineWidth: 0.5))
    }
}

// MARK: - Toggle (iOS switch matching prototype dimensions 44×26)
struct LXToggle: View {
    @Environment(\.theme) private var t
    @Binding var isOn: Bool

    var body: some View {
        ZStack(alignment: isOn ? .trailing : .leading) {
            RoundedRectangle(cornerRadius: 99)
                .fill(isOn ? t.accent : t.text4.tint(0.35))
                .frame(width: 44, height: 26)
            Circle()
                .fill(Color.white)
                .frame(width: 22, height: 22)
                .shadow(color: .black.opacity(0.2), radius: 1.5, y: 1)
                .padding(2)
        }
        .animation(.easeInOut(duration: 0.2), value: isOn)
        .contentShape(Rectangle())
        .onTapGesture { isOn.toggle() }
    }
}

// MARK: - Status bar (9:41 + signal/wifi/battery + dynamic island)
struct StatusBar: View {
    let dark: Bool
    var body: some View {
        let c: Color = dark ? .white : .black
        ZStack {
            HStack(alignment: .top) {
                Text("9:41")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(c)
                Spacer()
                HStack(spacing: 6) {
                    SignalBars(color: c)
                    WifiGlyph(color: c)
                    BatteryGlyph(color: c)
                }
            }
            .padding(.horizontal, 32)
            .padding(.top, 15)
            // Dynamic Island
            VStack {
                RoundedRectangle(cornerRadius: 22)
                    .fill(Color.black)
                    .frame(width: 124, height: 36)
                Spacer()
            }
            .padding(.top, 11)
        }
        .frame(height: 54)
    }
}

private struct SignalBars: View {
    let color: Color
    var body: some View {
        Canvas { ctx, _ in
            let s: CGFloat = 1
            func bar(_ x: CGFloat, _ y: CGFloat, _ h: CGFloat) {
                ctx.fill(Path(roundedRect: CGRect(x: x*s, y: y*s, width: 3*s, height: h*s), cornerRadius: 0.6),
                         with: .color(color))
            }
            bar(0,6,4); bar(4.5,4,6); bar(9,2,8); bar(13.5,0,10)
        }.frame(width: 18, height: 11)
    }
}
private struct WifiGlyph: View {
    let color: Color
    var body: some View {
        Canvas { ctx, _ in
            var p = Path()
            p.addArc(center: .init(x: 8, y: 10), radius: 7, startAngle: .degrees(220), endAngle: .degrees(320), clockwise: false)
            ctx.stroke(p, with: .color(color), lineWidth: 1.6)
            var p2 = Path()
            p2.addArc(center: .init(x: 8, y: 10), radius: 4, startAngle: .degrees(225), endAngle: .degrees(315), clockwise: false)
            ctx.stroke(p2, with: .color(color), lineWidth: 1.6)
            ctx.fill(Path(ellipseIn: CGRect(x: 6.7, y: 7.7, width: 2.6, height: 2.6)), with: .color(color))
        }.frame(width: 16, height: 11)
    }
}
private struct BatteryGlyph: View {
    let color: Color
    var body: some View {
        Canvas { ctx, _ in
            ctx.stroke(Path(roundedRect: CGRect(x: 0.5, y: 0.5, width: 22, height: 11), cornerRadius: 3),
                       with: .color(color.opacity(0.4)), lineWidth: 1)
            ctx.fill(Path(roundedRect: CGRect(x: 2, y: 2, width: 19, height: 8), cornerRadius: 1.8), with: .color(color))
            ctx.fill(Path(roundedRect: CGRect(x: 24, y: 4, width: 1.6, height: 4), cornerRadius: 0.8), with: .color(color.opacity(0.45)))
        }.frame(width: 26, height: 12)
    }
}

// MARK: - Home indicator
struct HomeIndicator: View {
    let dark: Bool
    var body: some View {
        VStack {
            Spacer()
            RoundedRectangle(cornerRadius: 99)
                .fill(dark ? Color.white : Color.black)
                .frame(width: 134, height: 5)
                .padding(.bottom, 8)
        }
        .frame(height: 28)
    }
}
