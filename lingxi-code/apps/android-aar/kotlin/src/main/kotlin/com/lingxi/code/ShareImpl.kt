// M8-P12 skeleton — Kotlin impl of the Rust-declared `SharingService` callback
// interface. M9 backs it with Intent.ACTION_SEND / chooser.
package com.lingxi.code

import com.lingxi.code.bindings.ShareError
import com.lingxi.code.bindings.SharePayload
import com.lingxi.code.bindings.ShareResult
import com.lingxi.code.bindings.SharingService

class AndroidShareImpl : SharingService {
    override suspend fun share(payload: SharePayload): ShareResult {
        // TODO(M9): build an ACTION_SEND Intent from payload.text / .url / .imageBytes.
        throw ShareError.Other("Unimplemented (M8 skeleton)")
    }
}
