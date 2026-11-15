// The plugin versions live here so the module file stays about the module.
// AGP 8.13 is the last 8.x line: it accepts compileSdk 36 and JDK 17 without
// the DSL rewrites AGP 9 asks for.
plugins {
    id("com.android.application") version "8.13.2" apply false
    id("org.jetbrains.kotlin.android") version "2.3.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.3.21" apply false
}
