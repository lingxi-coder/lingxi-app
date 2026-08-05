import Foundation
import Observation
import ObjectiveC
import os

/// Owns the user-selected app language ("" = follow system) and applies it by
/// swizzling Bundle.main's localizedString(forKey:value:table:) so both
/// String(localized:) and SwiftUI Text lookups resolve in the chosen language
/// without an app restart.
@Observable
@MainActor
final class LocalizationManager {

    // MARK: - Shared instance

    static let shared = LocalizationManager()

    // MARK: - Supported languages (order matches LanguagePage RadioList)

    static let supported: [(code: String, label: String)] = [
        ("",      "跟随系统"),
        ("zh-CN", "简体中文"),
        ("zh-TW", "繁體中文"),
        ("en-US", "English"),
        ("ja-JP", "日本語"),
        ("ko-KR", "한국어"),
    ]

    // MARK: - Observable state

    /// Persisted language code; "" means follow the system language.
    var language: String {
        didSet { persist(); apply() }
    }

    // MARK: - Private storage

    private let defaults: UserDefaults
    private static var swizzled = false

    // MARK: - Init

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let stored = defaults.string(forKey: "appLanguage") ?? ""
        self.language = Self.isSupported(stored) ? stored : ""
        Self.swizzleIfNeeded()
        apply()
    }

    // MARK: - Helpers

    static func isSupported(_ code: String) -> Bool {
        supported.contains { $0.code == code }
    }

    static func label(for code: String) -> String {
        supported.first { $0.code == code }?.label ?? "跟随系统"
    }

    /// Canonical locale for UI formatting (dates, numbers, etc.).
    func effectiveLocale() -> Locale {
        switch language {
        case "zh-CN": return Locale(identifier: "zh-Hans")
        case "zh-TW": return Locale(identifier: "zh-Hant")
        case "en-US": return Locale(identifier: "en")
        case "ja-JP": return Locale(identifier: "ja")
        case "ko-KR": return Locale(identifier: "ko")
        default:      return Locale.autoupdatingCurrent
        }
    }

    // MARK: - Persistence

    private func persist() {
        defaults.set(language.isEmpty ? nil : language, forKey: "appLanguage")
    }

    // MARK: - Apply language

    private func apply() {
        let canonical: String
        switch language {
        case "zh-CN": canonical = "zh-Hans"
        case "zh-TW": canonical = "zh-Hant"
        case "en-US": canonical = "en"
        case "ja-JP": canonical = "ja"
        case "ko-KR": canonical = "ko"
        default:      canonical = ""
        }
        // Write into the lock-protected cache consumed by the swizzled Bundle method (same file).
        Bundle._lxCurrentLanguageID.withLock { $0 = canonical }
        // Persist language preference for UIKit/system framework re-launches.
        if canonical.isEmpty {
            UserDefaults.standard.removeObject(forKey: "AppleLanguages")
        } else {
            UserDefaults.standard.set([canonical], forKey: "AppleLanguages")
        }
    }

    // MARK: - Swizzle setup (one-time)

    private static func swizzleIfNeeded() {
        guard !swizzled else { return }
        swizzled = true
        let cls = Bundle.self
        let orig = class_getInstanceMethod(cls, #selector(Bundle.localizedString(forKey:value:table:)))!
        let swz  = class_getInstanceMethod(cls, #selector(Bundle.lx_localizedString(forKey:value:table:)))!
        // Save the original IMP before exchange so the swizzled method can call it directly.
        Bundle._lxOriginalIMP = method_getImplementation(orig)
        method_exchangeImplementations(orig, swz)
    }
}

// MARK: - Bundle swizzle extension

extension Bundle {
    /// Cache of the canonical lproj directory ID currently in effect
    /// (e.g. "zh-Hans", "en"). Empty string means follow-system.
    /// Protected by an unfair lock: written on @MainActor from LocalizationManager.apply(),
    /// read on any thread from lx_localizedString (Foundation may call localizedString
    /// from background queues).
    static let _lxCurrentLanguageID = OSAllocatedUnfairLock<String>(initialState: "")

    /// Saved IMP of the original localizedString(forKey:value:table:) before the
    /// method exchange. Called directly (bypassing the swizzle) to perform the real
    /// lookup on a given bundle.
    static var _lxOriginalIMP: IMP?

    /// C-function signature matching Bundle.localizedString(forKey:value:table:)'s ObjC ABI:
    ///   NSString* (id self, SEL _cmd, NSString* key, NSString* value, NSString* table)
    typealias _LxLocalizedStringFn = @convention(c) (AnyObject, Selector, NSString, NSString?, NSString?) -> NSString

    // MARK: swizzled implementation

    @objc func lx_localizedString(forKey key: String, value: String?, table tableName: String?) -> String {
        // Only intercept the default Localizable table; forward all other tables untouched.
        guard tableName == "Localizable" || tableName == nil else {
            return Bundle._lxCallOriginal(on: self, key: key, value: value, table: tableName)
        }

        let canonicalID = Bundle._lxCurrentLanguageID.withLock { $0 }

        // Follow-system mode: forward original impl on self (no language override active).
        guard !canonicalID.isEmpty else {
            return Bundle._lxCallOriginal(on: self, key: key, value: value, table: tableName)
        }

        // Resolve the selected-language .lproj bundle.
        guard let lprojPath = Bundle.main.path(forResource: canonicalID, ofType: "lproj"),
              let langBundle = Bundle(path: lprojPath) else {
            // No matching lproj (catalog not yet populated): fall back to self.
            return Bundle._lxCallOriginal(on: self, key: key, value: value, table: tableName)
        }

        // Call original implementation on the target-language bundle.
        return Bundle._lxCallOriginal(on: langBundle, key: key, value: value, table: tableName)
    }

    // MARK: original IMP trampoline

    /// Calls the saved original IMP on the given bundle, bypassing the swizzle.
    static func _lxCallOriginal(on bundle: Bundle, key: String, value: String?, table: String?) -> String {
        guard let imp = _lxOriginalIMP else { return value ?? key }
        let fn = unsafeBitCast(imp, to: _LxLocalizedStringFn.self)
        let result = fn(
            bundle,
            #selector(Bundle.localizedString(forKey:value:table:)),
            key as NSString,
            value as NSString?,
            table as NSString?
        )
        return result as String
    }
}
