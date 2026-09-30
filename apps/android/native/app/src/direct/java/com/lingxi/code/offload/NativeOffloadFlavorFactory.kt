package com.lingxi.code.offload

/** Direct/Full composition: includes the complete native-offload command set. */
object NativeOffloadFlavorFactory {
    fun createHost(
        ports: NativeOffloadPorts = AndroidNativeOffloadPorts.create(),
        permissionGate: NativeOffloadPermissionGate =
            FailClosedNativeOffloadPermissionGate,
    ): NativeOffloadHost = DirectNativeOffloads.createHost(
        ports = ports,
        permissionGate = permissionGate,
    )
}
