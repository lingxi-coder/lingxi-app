import Foundation

enum TeamInvitation {
    static func code(_ input: String) -> String? {
        let value = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if validCode(value) { return value }
        guard let url = URLComponents(string: value), url.user == nil, url.password == nil, url.port == nil, url.fragment == nil,
              (url.scheme == "lingxi" && url.host == "team" && url.path == "/join") ||
                (url.scheme == "https" && url.host == "lingxios.net" && ["/open/team/join", "/teams/join"].contains(url.path)),
              let items = url.queryItems, items.count == 1, items[0].name == "code",
              let code = items[0].value, validCode(code) else { return nil }
        return code
    }
    static func validCode(_ value: String) -> Bool { value.range(of: "^[A-Za-z0-9_-]{32}$", options: .regularExpression) != nil }
    static func link(_ code: String) -> String { "https://lingxios.net/open/team/join?code=\(code)" }
}
