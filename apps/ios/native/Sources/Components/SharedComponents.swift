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

// MARK: - Semantic color blending
extension Color {
    /// Preserve dynamic appearance and contrast when blending semantic colors.
    func mix(with other: Color, amount: Double) -> Color {
        let first = UIColor(self)
        let second = UIColor(other)
        let fraction = max(0, min(1, amount))
        return Color(uiColor: UIColor { traits in
            var r1: CGFloat = 0, g1: CGFloat = 0, b1: CGFloat = 0, a1: CGFloat = 0
            var r2: CGFloat = 0, g2: CGFloat = 0, b2: CGFloat = 0, a2: CGFloat = 0
            first.resolvedColor(with: traits).getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
            second.resolvedColor(with: traits).getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
            return UIColor(
                red: r1 * fraction + r2 * (1 - fraction),
                green: g1 * fraction + g2 * (1 - fraction),
                blue: b1 * fraction + b2 * (1 - fraction),
                alpha: a1 * fraction + a2 * (1 - fraction)
            )
        })
    }

    func tint(_ amount: Double) -> Color {
        opacity(max(0, min(1, amount)))
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
            .font(.caption.weight(.medium))
            .padding(.horizontal, 9).padding(.vertical, 3)
            .foregroundColor(fg)
            .background(color != nil ? color!.tint(0.16) : t.surface)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6)
                .stroke(color != nil ? color!.tint(0.30) : t.border, lineWidth: 0.5))
    }
}

// MARK: - Native switch
private struct SettingsControlLabelKey: EnvironmentKey {
    static let defaultValue = ""
}

extension EnvironmentValues {
    var settingsControlLabel: String {
        get { self[SettingsControlLabelKey.self] }
        set { self[SettingsControlLabelKey.self] = newValue }
    }
}

struct LXToggle: View {
    @Environment(\.theme) private var t
    @Environment(\.settingsControlLabel) private var label
    @Binding var isOn: Bool

    var body: some View {
        Toggle(label, isOn: $isOn)
            .labelsHidden()
            .toggleStyle(.switch)
            .tint(t.accent)
            .fixedSize()
    }
}
