// Every plugin goes on one classpath here, so the Kotlin plugin can see
// the Android one when app/ applies both (plugins loaded by different
// projects can't see each other). The Android plugin is left out of a
// core-only build (-PcoreOnly), which then needs no Android SDK.
// Versions live in gradle/libs.versions.toml; modules apply plugins by
// id alone.
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        val kotlin = libs.versions.kotlin.get()
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:$kotlin")
        classpath("org.jetbrains.kotlin:kotlin-serialization:$kotlin")
        classpath("org.jetbrains.kotlin:compose-compiler-gradle-plugin:$kotlin")
        if (!providers.gradleProperty("coreOnly").isPresent) {
            classpath("com.android.tools.build:gradle:${libs.versions.agp.get()}")
        }
    }
}
