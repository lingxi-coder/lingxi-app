package com.lingxi.code.apps

import com.lingxi.code.model.SessionMode

internal const val APP_DEVELOPMENT_REQUIRES_CODE = "local_app_development_requires_code"
internal class AppDevelopmentRequiresCodeException : IllegalStateException(APP_DEVELOPMENT_REQUIRES_CODE)

internal fun requireCodeAuthoring(mode: SessionMode?) {
    if (mode != SessionMode.Code) throw AppDevelopmentRequiresCodeException()
}

internal fun authoringEntryAllowed(selected: SessionMode, owner: SessionMode?, source: SessionMode?,
    sessionConfirmed: Boolean): Boolean =
    selected == SessionMode.Code && owner == SessionMode.Code && source == SessionMode.Code && sessionConfirmed
