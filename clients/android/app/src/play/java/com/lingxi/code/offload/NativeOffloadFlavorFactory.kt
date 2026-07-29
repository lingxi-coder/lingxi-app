package com.lingxi.code.offload

/** Play/Store composition: privileged Direct-only handlers are absent. */
object NativeOffloadFlavorFactory {
    fun createHost(
        ports: NativeOffloadPorts = AndroidNativeOffloadPorts.create(),
        permissionGate: NativeOffloadPermissionGate =
            FailClosedNativeOffloadPermissionGate,
    ): NativeOffloadHost = NativeOffloadCatalog.createStoreHost(
        ports = ports,
        permissionGate = permissionGate,
    )
}
