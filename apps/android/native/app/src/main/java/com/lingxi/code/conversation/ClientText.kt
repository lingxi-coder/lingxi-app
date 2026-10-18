package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientTextDto

/** Build the UniFFI carrier without routing UTF-16 through UTF-8. */
internal fun String.toClientTextDto(): ClientTextDto =
    ClientTextDto(codeUnits = toCharArray().map { it.code.toUShort() })

/** Rebuild the editable Kotlin string without replacing isolated UTF-16 units. */
internal fun ClientTextDto.sourceText(): String = buildString(codeUnits.size) {
    codeUnits.forEach { append(it.toInt().toChar()) }
}

/** Lossy conversion belongs at the final UI/error-string boundary only. */
internal fun ClientTextDto.displayText(): String = buildString(codeUnits.size) {
    var index = 0
    while (index < codeUnits.size) {
        val unit = codeUnits[index].toInt()
        when {
            unit in 0xD800..0xDBFF && index + 1 < codeUnits.size &&
                codeUnits[index + 1].toInt() in 0xDC00..0xDFFF -> {
                append(unit.toChar())
                append(codeUnits[index + 1].toInt().toChar())
                index += 2
            }
            unit in 0xD800..0xDFFF -> {
                append('\uFFFD')
                index++
            }
            else -> {
                append(unit.toChar())
                index++
            }
        }
    }
}
