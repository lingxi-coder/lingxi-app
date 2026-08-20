pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}
plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // Vendored sherpa-onnx AAR (offline voice runtime) — no Maven publication
        // exists for k2-fsa/sherpa-onnx; the .aar lives in app/libs/ (gitignored,
        // fetched from the shared clients/voice/models.json manifest).
        flatDir { dirs("app/libs") }
    }
}

rootProject.name = "LingXiCode"
include(":app")
