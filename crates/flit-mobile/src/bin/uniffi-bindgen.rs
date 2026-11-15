//! The one binary CI and maintainers need: Kotlin bindings from the library.

fn main() {
    uniffi::uniffi_bindgen_main();
}
