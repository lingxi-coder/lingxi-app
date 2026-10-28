package com.lingxi.code.acceptance

/** Missing/false opts out; malformed explicit values fail instead of hiding a test. */
internal fun acceptanceEnabled(name: String, value: String?): Boolean {
    require(value == null || value == "true" || value == "false") { "$name must be true or false" }
    return value == "true"
}

/** Class/method selection and legacy fixture arguments must never become a silent skip. */
internal fun acceptanceRequested(target: String, selection: String?, legacyValues: List<String?>): Boolean {
    if (!selection.isNullOrBlank()) {
        val testClass = target.substringBefore('#')
        return selection.split(',').any { it.trim() == testClass || it.trim() == target }
    }
    return legacyValues.any { it != null }
}

internal fun requiredAcceptanceArg(name: String, value: String?): String {
    require(!value.isNullOrBlank()) { "Pass a non-empty $name for the selected acceptance" }
    return value
}
