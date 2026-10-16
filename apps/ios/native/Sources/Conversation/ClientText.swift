import Foundation

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

extension ClientTextDto {
    /// Lossy projection is reserved for a final SwiftUI / String sink.
    var displayText: String { String(decoding: codeUnits, as: UTF16.self) }
}

extension String {
    /// Carry this String's actual UTF-16 units into UniFFI without UTF-8 round-tripping.
    var clientTextDto: ClientTextDto { ClientTextDto(codeUnits: Array(utf16)) }
}
#endif
