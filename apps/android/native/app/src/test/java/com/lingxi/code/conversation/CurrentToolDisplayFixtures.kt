package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ToolHeaderDto
import com.lingxi.code.bindings.client.ToolResultDisplayDto
import com.lingxi.code.bindings.client.ToolVerbDto

internal fun currentToolHeaderFixture(
    label: String = "Read",
    verb: ToolVerbDto = ToolVerbDto.READ,
) = ToolHeaderDto(
    verb = verb,
    icon = null,
    label = label,
    primary = null,
    qualifier = null,
    count = null,
    subLine = null,
    title = label,
)

internal fun currentToolDisplayFixture(
    headline: String? = null,
    body: String? = null,
) = ToolResultDisplayDto(
    headline = headline,
    headlineKind = null,
    headlineArgs = emptyList(),
    diff = null,
    body = body,
    bodyLines = 0u,
    bodyTruncated = false,
    collapsed = false,
)
