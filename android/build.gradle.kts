// Plugin versions live in gradle/libs.versions.toml. The Kotlin plugins
// share one artifact, so all of them are put on the classpath here, once;
// the Android plugin is applied by app/ alone, so a core-only build never
// resolves it.
plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.kotlin.serialization) apply false
}
