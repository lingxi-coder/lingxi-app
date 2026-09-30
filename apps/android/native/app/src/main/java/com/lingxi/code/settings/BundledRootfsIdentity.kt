package com.lingxi.code.settings

import org.json.JSONObject

/** Immutable identity of the release archive selected by the packaged SDK assets. */
data class RootfsArtifactIdentity(
    val version: String,
    val abi: String,
    val sha256: String,
    val filename: String,
    val sizeBytes: Long,
) {
    init {
        require(version.matches(Regex("[A-Za-z0-9][A-Za-z0-9._-]*"))) { "Invalid bundled rootfs version" }
        require(sha256.matches(Regex("[a-f0-9]{64}"))) { "Invalid bundled rootfs SHA-256" }
        require(filename.matches(Regex("[A-Za-z0-9][A-Za-z0-9._-]*\\.tar\\.gz"))) { "Invalid bundled rootfs archive filename" }
        require(sizeBytes > 0) { "Invalid bundled rootfs archive size" }
        require(abi == "arm64-v8a" || abi == "x86_64") { "Unsupported bundled rootfs ABI: $abi" }
    }
}

internal fun bundledRootfsIdentity(pinsJson: String, manifestJson: String, abi: String): RootfsArtifactIdentity {
    val pins = JSONObject(pinsJson)
    check(pins.getInt("schema_version") == 2) { "Unsupported bundled rootfs pins schema" }
    check(pins.getString("source_toolchain_pins_sha256").matches(Regex("[a-f0-9]{64}"))) {
        "Bundled rootfs toolchain identity is missing or invalid"
    }
    val rootfs = pins.getJSONObject("rootfs")
    check(rootfs.has("release_archives")) { "Bundled rootfs requires release archive identities" }
    val releases = rootfs.getJSONObject("release_archives")
    check(releases.has(abi)) { "No bundled release rootfs for ABI $abi" }
    val release = releases.getJSONObject(abi)
    val identity = RootfsArtifactIdentity(
        version = rootfs.getString("version"), abi = abi,
        sha256 = release.getString("sha256"), filename = release.getString("filename"),
        sizeBytes = release.getLong("size_bytes"),
    )
    val manifest = JSONObject(manifestJson)
    val manifestAbi = if (abi == "arm64-v8a") "arm64" else "x86_64"
    check(manifest.getString("platform") == "android" && manifest.getString("runtime") == "android-proot") {
        "Bundled rootfs manifest targets a different runtime"
    }
    check(manifest.getString("abi") == manifestAbi && manifest.getString("rootfs_version") == identity.version) {
        "Bundled rootfs manifest version/ABI differs from release pins"
    }
    val archive = manifest.getJSONObject("archive")
    check(archive.getString("sha256") == identity.sha256 &&
        archive.getString("filename") == identity.filename && archive.getLong("size_bytes") == identity.sizeBytes) {
        "Bundled rootfs manifest archive differs from release pins"
    }
    return identity
}
