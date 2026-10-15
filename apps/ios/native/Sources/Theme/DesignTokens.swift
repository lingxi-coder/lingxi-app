import SwiftUI
import UIKit

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

    /// Runtime `oklch(L C H / a)` → sRGB. Unlike the pre-converted tokens above,
    /// the voice orb (OrbCanvas) computes colors on the fly — hue shifts, per-
    /// particle hues, gradient stops — so it needs a live OKLCH→sRGB conversion.
    /// `l` is 0…1 lightness (pass 0.85 for the prototype's `85%`), `c` chroma,
    /// `h` hue in degrees. Implements OKLab→linear-sRGB + the sRGB transfer fn.
    init(okl l: Double, _ c: Double, _ h: Double, _ alpha: Double = 1) {
        let hr = h * .pi / 180
        let a = c * cos(hr)
        let b = c * sin(hr)
        let l_ = l + 0.3963377774 * a + 0.2158037573 * b
        let m_ = l - 0.1055613458 * a - 0.0638541728 * b
        let s_ = l - 0.0894841775 * a - 1.2914855480 * b
        let lc = l_ * l_ * l_, mc = m_ * m_ * m_, sc = s_ * s_ * s_
        let rl =  4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc
        let gl = -1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc
        let bl = -0.0041960863 * lc - 0.7034186147 * mc + 1.7076147010 * sc
        func enc(_ x: Double) -> Double {
            let v = max(0, min(1, x))
            return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055
        }
        self.init(.sRGB, red: enc(rl), green: enc(gl), blue: enc(bl), opacity: alpha)
    }
}

/// A resolved palette for one appearance (dark or light).
/// Uses the native iOS grouped surface and semantic color hierarchy.
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

    // MARK: Diff + code rendering
    //
    /// Which appearance this palette is. `SyntaxPalette` needs the ramp and a
    /// segment's baked `rgb` is only usable on the dark ground it was resolved
    /// against, so the palette carries its own identity rather than making every
    /// call site read `@Environment(\.colorScheme)`.
    let isDark: Bool
    // Diff row/word backgrounds. These are deliberately NOT on the wire: the
    // terminal's blends are alpha-over-BLACK and are unreadable over a light
    // ground, so each client derives its own from `kind` / `emph`.
    let diffAddBg: Color
    let diffRemoveBg: Color
    let diffAddWordBg: Color
    let diffRemoveWordBg: Color
    let diffAddGutter: Color
    let diffRemoveGutter: Color

}

/// Helper carrying the ambient gradient color + the window bg fade target.
struct LinearGradientStops {
    let top: Color
    let bottom: Color
}

enum DesignTokens {
    static let dark = nativePalette(isDark: true)
    static let light = nativePalette(isDark: false)

    static func palette(dark isDark: Bool) -> Palette { isDark ? dark : light }

    /// Semantic UIKit colors follow appearance, accessibility contrast and
    /// elevated presentation. All screens use this same surface hierarchy.
    private static func nativePalette(isDark: Bool) -> Palette {
        Palette(
            appBg: Color(uiColor: .systemGroupedBackground),
            windowBg: Color(uiColor: .systemGroupedBackground),
            sidebarBg: Color(uiColor: .systemGroupedBackground),
            surface: Color(uiColor: .secondarySystemGroupedBackground),
            surfaceHover: Color(uiColor: .tertiarySystemFill),
            surfaceActive: Color(uiColor: .secondarySystemFill),
            border: Color(uiColor: .separator),
            borderStrong: Color(uiColor: .opaqueSeparator),
            text: Color(uiColor: .label),
            text2: Color(uiColor: .secondaryLabel),
            text3: Color(uiColor: .secondaryLabel),
            text4: Color(uiColor: .secondaryLabel),
            accent: Color(uiColor: .systemBlue),
            accent2: Color(uiColor: .systemPurple),
            accent3: Color(uiColor: .systemTeal),
            ok: Color(uiColor: .systemGreen),
            composerBg: Color(uiColor: .secondarySystemGroupedBackground),
            ambient: LinearGradientStops(top: .clear, bottom: .clear),
            statusConnected: Color(uiColor: .systemGreen),
            statusIdle: Color(uiColor: .tertiaryLabel),
            statusTesting: Color(uiColor: .systemOrange),
            statusError: Color(uiColor: .systemRed),
            danger: Color(uiColor: .systemRed),
            isDark: isDark,
            diffAddBg: Color.green.opacity(isDark ? 0.13 : 0.11),
            diffRemoveBg: Color.red.opacity(isDark ? 0.12 : 0.09),
            diffAddWordBg: Color.green.opacity(isDark ? 0.32 : 0.26),
            diffRemoveWordBg: Color.red.opacity(isDark ? 0.30 : 0.22),
            diffAddGutter: Color(uiColor: .systemGreen),
            diffRemoveGutter: Color(uiColor: .systemRed)
        )
    }
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
        .init(id: "oklch(70% 0.18 268)", name: String(localized: "settings_accent_indigo"), color: Color(uiColor: .systemIndigo)),
        .init(id: "oklch(70% 0.18 320)", name: String(localized: "settings_accent_rose"), color: Color(uiColor: .systemPink)),
        .init(id: "oklch(72% 0.16 195)", name: String(localized: "settings_accent_cyan"), color: Color(uiColor: .systemTeal)),
        .init(id: "oklch(72% 0.16 155)", name: String(localized: "settings_accent_teal"), color: Color(uiColor: .systemGreen)),
        .init(id: "oklch(74% 0.16 75)",  name: String(localized: "settings_accent_amber"), color: Color(uiColor: .systemOrange)),
        .init(id: "oklch(70% 0.20 30)",  name: String(localized: "settings_accent_brick"), color: Color(uiColor: .systemRed)),
    ]
    static func color(for id: String) -> Color {
        all.first(where: { $0.id == id })?.color ?? all[0].color
    }
}
