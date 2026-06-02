import SwiftUI

// MARK: - oklch → sRGB helper
//
// SwiftUI has no oklch color space. Every token below was converted from its
// original CSS `oklch(L C H)` value to sRGB (D65) using the standard
// OKLab→linear-sRGB matrix + sRGB gamma, then expressed via
// `Color(.sRGB, red:green:blue:opacity:)`. The original oklch string is kept
// in a trailing comment for traceability.

extension Color {
    /// Build an sRGB color. Values are pre-converted from oklch.
    init(srgb r: Double, _ g: Double, _ b: Double, _ a: Double = 1) {
        self.init(.sRGB, red: r, green: g, blue: b, opacity: a)
    }
}

/// A resolved palette for one appearance (dark or light).
/// Mirrors the `tokens(dark)` factory in the prototype exactly.
struct Palette {
    let appBg: Color
    let windowBg: Color
    let sidebarBg: Color
    let surface: Color
    let surfaceHover: Color
    let surfaceActive: Color
    let border: Color
    let borderStrong: Color
    let text: Color
    let text2: Color
    let text3: Color
    let text4: Color
    var accent: Color          // var → overridable by Appearance accent picker
    let accent2: Color
    let accent3: Color
    let ok: Color
    let composerBg: Color
    /// The radial ambient gradient painted behind the chat view.
    let ambient: LinearGradientStops

    // Status dots (shared across dark/light in the prototype)
    let statusConnected: Color   // oklch(70% 0.15 155)
    let statusIdle: Color        // == text4
    let statusTesting: Color     // oklch(75% 0.15 75)
    let statusError: Color       // oklch(65% 0.20 25)
    let danger: Color            // oklch(65% 0.20 25)
}

/// Helper carrying the ambient gradient color + the window bg fade target.
struct LinearGradientStops {
    let top: Color
    let bottom: Color
}

enum DesignTokens {

    // MARK: Dark palette  (tokens(true))
    static let dark = Palette(
        appBg:         Color(srgb: 0.0392, 0.0392, 0.0470),                 // #0a0a0c
        windowBg:      Color(srgb: 0.0358, 0.0430, 0.0640),                 // oklch(15% 0.012 270)
        sidebarBg:     Color(srgb: 0.0226, 0.0279, 0.0471),                 // oklch(13% 0.012 270)
        surface:       Color(srgb: 0.0585, 0.0680, 0.0955),                 // oklch(18% 0.015 270)
        surfaceHover:  Color(srgb: 0.0897, 0.1029, 0.1415),                 // oklch(22% 0.020 270)
        surfaceActive: Color(srgb: 0.1372, 0.1579, 0.2193),                 // oklch(28% 0.030 270)
        border:        Color(srgb: 0.1485, 0.1594, 0.1902, 0.6),            // oklch(28% 0.015 270 / 0.6)
        borderStrong:  Color(srgb: 0.2130, 0.2283, 0.2717, 0.8),            // oklch(35% 0.020 270 / 0.8)
        text:          Color(srgb: 0.9423, 0.9475, 0.9616),                 // oklch(96% 0.005 270)
        text2:         Color(srgb: 0.6303, 0.6445, 0.6837),                 // oklch(72% 0.015 270)
        text3:         Color(srgb: 0.3931, 0.4103, 0.4583),                 // oklch(52% 0.020 270)
        text4:         Color(srgb: 0.2434, 0.2591, 0.3034),                 // oklch(38% 0.020 270)
        accent:        Color(srgb: 0.4340, 0.5865, 1.0000),                 // oklch(70% 0.18 268)
        accent2:       Color(srgb: 0.8090, 0.4552, 0.8891),                 // oklch(70% 0.18 320)
        accent3:       Color(srgb: 0.0000, 0.7601, 0.7664),                 // oklch(72% 0.16 195)
        ok:            Color(srgb: 0.2305, 0.7257, 0.4545),                 // oklch(70% 0.15 155)
        composerBg:    Color(srgb: 0.0756, 0.0855, 0.1137),                 // oklch(20% 0.015 270)
        ambient:       LinearGradientStops(
                          top: Color(srgb: 0.1700, 0.1300, 0.4200, 0.20),   // oklch(40% 0.15 268 / .20)
                          bottom: .clear),
        statusConnected: Color(srgb: 0.2305, 0.7257, 0.4545),              // oklch(70% 0.15 155)
        statusIdle:      Color(srgb: 0.2434, 0.2591, 0.3034),              // text4
        statusTesting:   Color(srgb: 0.8959, 0.6198, 0.1315),              // oklch(75% 0.15 75)
        statusError:     Color(srgb: 0.9436, 0.3038, 0.2990),              // oklch(65% 0.20 25)
        danger:          Color(srgb: 0.9436, 0.3038, 0.2990)               // oklch(65% 0.20 25)
    )

    // MARK: Light palette  (tokens(false))
    static let light = Palette(
        appBg:         Color(srgb: 0.8118, 0.7882, 0.8471),                 // #cfc9d8
        windowBg:      Color(srgb: 0.9804, 0.9725, 0.9608),                 // #faf8f5
        sidebarBg:     Color(srgb: 0.9423, 0.9475, 0.9616),                 // oklch(96% 0.005 270)
        surface:       Color(srgb: 1.0, 1.0, 1.0),                          // #ffffff
        surfaceHover:  Color(srgb: 0.9392, 0.9475, 0.9700),                 // oklch(96% 0.008 270)
        surfaceActive: Color(srgb: 0.8751, 0.8952, 0.9508),                 // oklch(92% 0.020 270)
        border:        Color(srgb: 0.8361, 0.8441, 0.8662),                 // oklch(88% 0.008 270)
        borderStrong:  Color(srgb: 0.7563, 0.7681, 0.8005),                 // oklch(82% 0.012 270)
        text:          Color(srgb: 0.0736, 0.0852, 0.1191),                 // oklch(20% 0.018 270)
        text2:         Color(srgb: 0.2640, 0.2799, 0.3248),                 // oklch(40% 0.020 270)
        text3:         Color(srgb: 0.4608, 0.4785, 0.5279),                 // oklch(58% 0.020 270)
        text4:         Color(srgb: 0.6062, 0.6203, 0.6592),                 // oklch(70% 0.015 270)
        accent:        Color(srgb: 0.1913, 0.3056, 0.8696),                 // oklch(50% 0.22 268)
        accent2:       Color(srgb: 0.6518, 0.1966, 0.7474),                 // oklch(55% 0.22 320)
        accent3:       Color(srgb: 0.0000, 0.5200, 0.5362),                 // oklch(52% 0.18 195)
        ok:            Color(srgb: 0.0000, 0.5455, 0.2695),                 // oklch(55% 0.16 155)
        composerBg:    Color(srgb: 1.0, 1.0, 1.0),                          // #ffffff
        ambient:       LinearGradientStops(
                          top: Color(srgb: 0.7400, 0.7900, 0.9700, 0.45),   // oklch(80% 0.10 268 / .45)
                          bottom: .clear),
        statusConnected: Color(srgb: 0.2305, 0.7257, 0.4545),
        statusIdle:      Color(srgb: 0.6062, 0.6203, 0.6592),
        statusTesting:   Color(srgb: 0.8959, 0.6198, 0.1315),
        statusError:     Color(srgb: 0.9436, 0.3038, 0.2990),
        danger:          Color(srgb: 0.9436, 0.3038, 0.2990)
    )

    static func palette(dark isDark: Bool) -> Palette { isDark ? dark : light }
}

// MARK: - Accent options (Appearance screen) -------------------------------

/// The 6 accent swatches from the prototype's Appearance page.
struct AccentOption: Identifiable, Equatable {
    let id: String      // the original oklch string (used as the selection key)
    let name: String
    let color: Color
}

enum Accents {
    static let all: [AccentOption] = [
        .init(id: "oklch(70% 0.18 268)", name: "靛紫", color: Color(srgb: 0.4340, 0.5865, 1.0000)),
        .init(id: "oklch(70% 0.18 320)", name: "玫红", color: Color(srgb: 0.8090, 0.4552, 0.8891)),
        .init(id: "oklch(72% 0.16 195)", name: "青蓝", color: Color(srgb: 0.0000, 0.7601, 0.7664)),
        .init(id: "oklch(72% 0.16 155)", name: "青绿", color: Color(srgb: 0.2085, 0.7571, 0.4656)),
        .init(id: "oklch(74% 0.16 75)",  name: "琥珀", color: Color(srgb: 0.8960, 0.6013, 0.0000)),
        .init(id: "oklch(70% 0.20 30)",  name: "砖红", color: Color(srgb: 1.0000, 0.3802, 0.3010)),
    ]
    static func color(for id: String) -> Color {
        all.first(where: { $0.id == id })?.color ?? all[0].color
    }
}
