package com.lingxi.code.localapps

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.PluginCommandDto
import com.lingxi.code.conversation.ConversationSource

/**
 * The id of the engine plugin that backs Local Apps (the plugin's own manifest name).
 *
 * This client carries no Local App UI: no library, no workspace, no approval sheet, no widget. The plugin's tools are
 * therefore switched off here rather than left offered to a model that nothing on screen could answer.
 */
internal const val LOCAL_APP_PLUGIN_ID = "lingxi-local-app"

/**
 * Ask the engine to unload the Local App plugin. Idempotent, and sent on every (re)connect because the engine is
 * rebuilt then. A refusal or a source that cannot take commands is not an error: the plugin simply stays as it was.
 */
internal suspend fun ConversationSource.disableLocalAppPlugin() {
    runCatching {
        submitClientCommand(
            ClientCommand.PluginCommand(
                PluginCommandDto.SetEnabled(pluginId = LOCAL_APP_PLUGIN_ID, enabled = false),
            ),
        )
    }
}
