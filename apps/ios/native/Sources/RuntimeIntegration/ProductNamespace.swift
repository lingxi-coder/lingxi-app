import Foundation

private final class ProductNamespaceBundleAnchor: NSObject {}

/// Namespace values generated from the Cargo-locked branding package.
/// This file is compiled into both the app and its independent UI-test bundle.
enum ProductNamespace {
    private struct Values: Decodable {
        let version: Int
        let envPrefix: String
    }

    private static let values: Values = {
        let bundle = Bundle(for: ProductNamespaceBundleAnchor.self)
        guard let url = bundle.url(forResource: "product-namespace", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let values = try? JSONDecoder().decode(Values.self, from: data),
              values.version == 1, !values.envPrefix.isEmpty else {
            fatalError("The generated product namespace is unavailable")
        }
        return values
    }()

    static func environmentKey(_ suffix: String) -> String {
        values.envPrefix + suffix
    }
}
