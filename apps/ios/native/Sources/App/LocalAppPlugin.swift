import Foundation

/// The engine's Local App plugin.
///
/// This client carries no Local App UI: no library, no workspace, no approval sheet, no widget. The plugin's tools are
/// therefore switched off here rather than left offered to a model that nothing on screen could answer.
enum LocalAppPlugin {
    /// The plugin's own manifest name, the key the engine's `enabledPlugins` setting uses.
    static let identifier = "lingxi-local-app"

    /// Ask the engine to unload the plugin. Idempotent, and sent on every bootstrap because the engine is rebuilt
    /// then. A refusal is not an error: the plugin simply stays as it was.
    @MainActor
    static func disable(on source: any ConversationSource) async {
        #if canImport(harness_runtimeFFI)
            try? await source.submitEngineCommand(
                .pluginCommand(command: .setEnabled(pluginId: identifier, enabled: false))
            )
        #endif
    }
}
