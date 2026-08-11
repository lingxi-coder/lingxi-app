package com.lingxi.code.theme

import androidx.compose.ui.graphics.Color
import com.lingxi.code.conversation.SyntaxClassUi

/**
 * Syntax colors for the structured-diff renderer, one set per appearance.
 *
 * ### Why this exists at all
 *
 * Every [com.lingxi.code.conversation.CodeSegmentUi] arrives with BOTH a
 * [SyntaxClassUi] and an optional `rgb` packed `0x00RRGGBB`. The `rgb` is the
 * TERMINAL's resolved foreground — baked against one dark theme. Painting it on
 * the light palette is unreadable, and it cannot follow the app's runtime theme
 * toggle. So the class is authoritative here and `rgb` is used ONLY as the
 * dark-mode fallback for [SyntaxClassUi.Plain], where the engine had no class to
 * assign and the terminal's own default is the best information available.
 *
 * The two sets below are deliberately conventional (a One-Dark-family dark set,
 * a GitHub-light-family light set) rather than derived from the brand accents:
 * a diff has to read like code, and brand hues collapse the class distinctions
 * that make it readable.
 */
@Suppress("MemberVisibilityCanBePrivate")
data class SyntaxPalette(
    val plain: Color,
    val keyword: Color,
    val typeName: Color,
    val function: Color,
    val stringLit: Color,
    val number: Color,
    val comment: Color,
    val punctuation: Color,
    val operator: Color,
    val variable: Color,
    val constant: Color,
    val attribute: Color,
) {
    /** The color for one segment's class. Exhaustive — a new class is a compile error. */
    fun color(syntax: SyntaxClassUi): Color = when (syntax) {
        SyntaxClassUi.Plain -> plain
        SyntaxClassUi.Keyword -> keyword
        SyntaxClassUi.TypeName -> typeName
        SyntaxClassUi.Function -> function
        SyntaxClassUi.StringLit -> stringLit
        SyntaxClassUi.Number -> number
        SyntaxClassUi.Comment -> comment
        SyntaxClassUi.Punctuation -> punctuation
        SyntaxClassUi.Operator -> operator
        SyntaxClassUi.Variable -> variable
        SyntaxClassUi.Constant -> constant
        SyntaxClassUi.Attribute -> attribute
    }

    companion object {
        /** One-Dark family — legible on [Palette.diffSurface] in the dark appearance. */
        val dark = SyntaxPalette(
            plain = Color(0xFFC8CEDA),
            keyword = Color(0xFFC678DD),
            typeName = Color(0xFFE5C07B),
            function = Color(0xFF61AFEF),
            stringLit = Color(0xFF98C379),
            number = Color(0xFFD19A66),
            comment = Color(0xFF7F848E),
            punctuation = Color(0xFFABB2BF),
            operator = Color(0xFF56B6C2),
            variable = Color(0xFFE06C75),
            constant = Color(0xFFD19A66),
            attribute = Color(0xFFE5C07B),
        )

        /** GitHub-light family — legible on [Palette.diffSurface] in the light appearance. */
        val light = SyntaxPalette(
            plain = Color(0xFF24292F),
            keyword = Color(0xFFCF222E),
            typeName = Color(0xFF953800),
            function = Color(0xFF8250DF),
            stringLit = Color(0xFF0A3069),
            number = Color(0xFF0550AE),
            comment = Color(0xFF6E7781),
            punctuation = Color(0xFF3D444D),
            operator = Color(0xFF0550AE),
            variable = Color(0xFF953800),
            constant = Color(0xFF0550AE),
            attribute = Color(0xFF116329),
        )

        fun forAppearance(isDark: Boolean): SyntaxPalette = if (isDark) dark else light
    }
}

/** The syntax set matching this brand [Palette]'s appearance. */
val Palette.syntax: SyntaxPalette get() = SyntaxPalette.forAppearance(isDark)

/**
 * Resolve one segment's foreground.
 *
 * Class-first, always. The wire `rgb` is consulted ONLY in the dark appearance
 * for an unclassified ([SyntaxClassUi.Plain]) run, because that is the one case
 * where the class carries no information and the terminal's own resolution was
 * computed for a dark ground anyway.
 */
fun Palette.segmentColor(syntax: SyntaxClassUi, rgb: Int?): Color {
    if (isDark && syntax == SyntaxClassUi.Plain && rgb != null) {
        return Color(0xFF000000.toInt() or (rgb and 0x00FFFFFF))
    }
    return this.syntax.color(syntax)
}
