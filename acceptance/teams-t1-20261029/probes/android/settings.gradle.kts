pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement {
    repositories { google(); mavenCentral() }
    versionCatalogs { create("libs") { from(files("../../../../apps/android/native/gradle/libs.versions.toml")) } }
}
rootProject.name = "TeamsMembersProbe"
