package com.lingxi.code.settings

import io.lingxi.mobilelinux.RootfsInstaller
import io.lingxi.mobilelinux.BrandRuntimeProfile
import java.io.File

/** Host call boundary; archive validation and activation staging belong to the SDK. */
internal object BundledRootfsInstaller {
    fun stage(
        managedRoot: File,
        expectedRootfsVersion: String,
        expectedArchiveSha: String,
        expectedManifestAbi: String,
        expectedBrandProfile: BrandRuntimeProfile,
        archiveName: String,
        manifestJson: String,
        sbomJson: String,
        copyArchive: (File) -> Unit,
        persistManifest: ((String) -> Unit)? = null,
    ): RootfsInstaller.StageResult = RootfsInstaller.stage(
        managedRoot = managedRoot,
        expectedRootfsVersion = expectedRootfsVersion,
        expectedArchiveSha = expectedArchiveSha,
        expectedManifestAbi = expectedManifestAbi,
        expectedBrandProfile = expectedBrandProfile,
        archiveName = archiveName,
        manifestJson = manifestJson,
        sbomJson = sbomJson,
        copyArchive = copyArchive,
        persistManifest = persistManifest,
    )
}
