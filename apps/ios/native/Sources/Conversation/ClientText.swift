import Foundation
import CoreFoundation

/// Native display bridge. CF's mutable UTF-16 storage retains an unfinished
/// pair between events; each returned Swift String is a display snapshot.
/// Model input and persistence continue to use ClientTextDto's exact units.
final class ClientTextBuffer {
    private let storage = CFStringCreateMutable(nil, 0)!

    init(_ displayPrefix: String = "") {
        append(Array(displayPrefix.utf16))
    }

    @discardableResult
    func append(_ units: [UInt16]) -> String {
        if !units.isEmpty {
            units.withUnsafeBufferPointer { pointer in
                CFStringAppendCharacters(storage, pointer.baseAddress, pointer.count)
            }
        }
        return storage as String
    }
}

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
