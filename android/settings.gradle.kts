// The phone remote (ROADMAP 2.2). `core` is plain Kotlin: the protocol,
// the connection and the overview geometry, tested on any machine with a
// JDK. `app` is the Android app, which needs the Android SDK; build with
// -PcoreOnly to leave it out.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "daw-remote"

include(":core")
if (!providers.gradleProperty("coreOnly").isPresent) {
    include(":app")
}
