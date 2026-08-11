// SyntaxPalette.swift — `ConversationSyntaxClass` → a readable Color.
//
// The wire carries BOTH a semantic `class` and a terminal-resolved `rgb`. Only
// the class is portable: `rgb` was baked against ONE dark terminal theme, so
// painting it on the light palette is unreadable AND it cannot follow the
// runtime theme toggle. This table is the client's own ramp, one per
// appearance, and it is the only thing a diff/code renderer may ask for a
// foreground color.
//
// `rgb` is used in exactly one place — a `plain` segment in DARK mode, where the
// terminal's own resolution is both valid and strictly more informative than
// "just use the body color" (it is how ANSI-colored tool output keeps its
// colors). Everything else goes through the class.

import SwiftUI

enum SyntaxPalette {

    /// Foreground for one pre-split segment, honoring the one `rgb` exception.
    static func foreground(for segment: ConversationCodeSegment, in palette: Palette) -> Color {
        if segment.syntax == .plain, palette.isDark, let rgb = segment.rgb {
            return Color(packedRGB: rgb)
        }
        return color(for: segment.syntax, in: palette)
    }

    /// The appearance-correct color for a syntax class.
    static func color(for syntax: ConversationSyntaxClass, in palette: Palette) -> Color {
        palette.isDark ? dark(syntax) : light(syntax)
    }

    // MARK: dark ramp
    //
    // Tuned against `windowBg` oklch(15% 0.012 270): saturated but not neon, and
    // every hue stays above the WCAG-AA large-text threshold on that ground.
    private static func dark(_ syntax: ConversationSyntaxClass) -> Color {
        switch syntax {
        case .plain:       return Color(srgb: 0.8600, 0.8700, 0.8900)
        case .keyword:     return Color(srgb: 0.8090, 0.4552, 0.8891)   // accent2 family
        case .typeName:    return Color(srgb: 0.4600, 0.7900, 0.9700)
        case .function:    return Color(srgb: 0.4340, 0.5865, 1.0000)   // accent family
        case .stringLit:   return Color(srgb: 0.4700, 0.8200, 0.5300)
        case .number:      return Color(srgb: 0.9600, 0.6900, 0.4200)
        case .comment:     return Color(srgb: 0.4400, 0.4700, 0.5400)
        case .punctuation: return Color(srgb: 0.6200, 0.6500, 0.7100)
        case .op:          return Color(srgb: 0.9000, 0.6300, 0.7400)
        case .variable:    return Color(srgb: 0.8600, 0.8700, 0.8900)
        case .constant:    return Color(srgb: 0.9700, 0.6100, 0.5200)
        case .attribute:   return Color(srgb: 0.9200, 0.8100, 0.4600)
        }
    }

    // MARK: light ramp
    //
    // Tuned against `surface` #ffffff / `windowBg` #faf8f5: darker and less
    // chromatic than the dark ramp so nothing washes out on paper-white.
    private static func light(_ syntax: ConversationSyntaxClass) -> Color {
        switch syntax {
        case .plain:       return Color(srgb: 0.1200, 0.1300, 0.1700)
        case .keyword:     return Color(srgb: 0.5600, 0.1200, 0.6300)
        case .typeName:    return Color(srgb: 0.0700, 0.3800, 0.5600)
        case .function:    return Color(srgb: 0.1600, 0.2600, 0.7400)
        case .stringLit:   return Color(srgb: 0.0700, 0.4600, 0.2400)
        case .number:      return Color(srgb: 0.6400, 0.3400, 0.0500)
        case .comment:     return Color(srgb: 0.4400, 0.4700, 0.5200)
        case .punctuation: return Color(srgb: 0.3200, 0.3400, 0.3900)
        case .op:          return Color(srgb: 0.6300, 0.2000, 0.3600)
        case .variable:    return Color(srgb: 0.1200, 0.1300, 0.1700)
        case .constant:    return Color(srgb: 0.6800, 0.2400, 0.1400)
        case .attribute:   return Color(srgb: 0.5400, 0.4200, 0.0600)
        }
    }
}

extension Color {
    /// Unpack a `0x00RRGGBB` wire color. Only ever applied on the dark palette —
    /// see `SyntaxPalette.foreground`.
    init(packedRGB value: UInt32) {
        self.init(
            .sRGB,
            red: Double((value >> 16) & 0xFF) / 255,
            green: Double((value >> 8) & 0xFF) / 255,
            blue: Double(value & 0xFF) / 255,
            opacity: 1
        )
    }
}
