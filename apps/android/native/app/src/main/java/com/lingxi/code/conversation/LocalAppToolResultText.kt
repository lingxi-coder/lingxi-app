package com.lingxi.code.conversation

import com.lingxi.code.apps.APP_DEVELOPMENT_REQUIRES_CODE

// Live tool errors use the bare reason; restored tool results may prefix it with "Error: ".
// A token elsewhere in an error (for example, in a file path) is not that reason.
private val codeRequiredReason = Regex("^\\s*(?:Error:\\s*)?${Regex.escape(APP_DEVELOPMENT_REQUIRES_CODE)}(?=$|[:\\s])")

internal fun isLocalAppCodeRequired(tool: String, status: AgentToolStatus, headline: String?, body: String?): Boolean =
    tool.equals("LocalApp", ignoreCase = true) && status == AgentToolStatus.Failed &&
        listOfNotNull(headline, body).any(codeRequiredReason::containsMatchIn)
