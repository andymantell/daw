// Plugin versions live in gradle/libs.versions.toml. The Android plugins
// are applied by app/ alone, so a core-only build never resolves them.
plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.kotlin.serialization) apply false
}
