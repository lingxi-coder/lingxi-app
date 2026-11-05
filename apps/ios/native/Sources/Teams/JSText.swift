import Foundation

/// JavaScript string semantics the Desktop rules rely on (`trim()`, `\s+`), shared by the pure Teams logic so
/// phone and Desktop agree on what is blank. Whitespace is JavaScript's `\s` (`AvatarIdentity.isSpace`).
enum JSText {
    /// `String.prototype.trim`.
    static func trim(_ text: String) -> String {
        var scalars = Substring(text).unicodeScalars
        while let first = scalars.first, AvatarIdentity.isSpace(first) { scalars.removeFirst() }
        while let last = scalars.last, AvatarIdentity.isSpace(last) { scalars.removeLast() }
        return String(String.UnicodeScalarView(scalars))
    }

    /// `text.replace(/\s+/gu, ' ')`: every run of whitespace becomes one space.
    static func collapseSpaces(_ text: String) -> String {
        var result = String.UnicodeScalarView()
        var inRun = false
        for scalar in text.unicodeScalars {
            if AvatarIdentity.isSpace(scalar) {
                if !inRun { result.append(" ") }
                inRun = true
            } else {
                result.append(scalar)
                inRun = false
            }
        }
        return String(result)
    }

    /// `text.trim().length === 0`.
    static func isBlank(_ text: String) -> Bool {
        text.unicodeScalars.allSatisfy(AvatarIdentity.isSpace)
    }
}
