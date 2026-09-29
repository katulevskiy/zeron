pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "Zeron"
// app: Compose UI over the Rust core (zeron-mobile via UniFFI).
// runtime: the on-device engine — proot guest bootstrap + RuntimeService.
include(":app", ":runtime")
