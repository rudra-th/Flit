# Flit for Android

The phone as a peer: it announces itself under its own name, and it appears on
the computer with the fingerprint that key pins. Sending and receiving comes
next; this build deliberately has no button that would do nothing.

## What is here

| Path | What it is |
| --- | --- |
| `app/src/main/java/dev/flit/android/HomeViewModel.kt` | starts and stops the engine, holds the multicast lock, polls for peers |
| `app/src/main/java/dev/flit/android/HomeScreen.kt` | the one screen: this phone, whoever is nearby |
| `app/src/main/java/dev/flit/android/ffi/` | **generated**, ignored by git — UniFFI's Kotlin for `crates/flit-mobile` |
| `app/src/main/jniLibs/` | **generated**, ignored by git — `libflit_mobile.so` from `cargo ndk` |

Everything below the UI is the same Rust that runs on the desktop. The Kotlin
never sees a manifest, a socket or a certificate.

## Building

You need the Android SDK (platform 36, build-tools 36), the NDK, JDK 17 and a
Rust toolchain with the two Android targets. The Gradle wrapper is committed, so
`./gradlew` is all the Android side needs.

```sh
# once
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk

# bindings, from a host build (debug: the release one is stripped)
cargo build -p flit-mobile --features cli
cargo run -p flit-mobile --features cli --bin uniffi-bindgen -- \
  generate --library target/debug/libflit_mobile.so \
  --language kotlin \
  --out-dir apps/android/app/src/main/java \
  --no-format

# the libraries the APK loads
cargo ndk -t arm64-v8a -t x86_64 -P 26 \
  -o apps/android/app/src/main/jniLibs \
  build --release -p flit-mobile

# the APK
cd apps/android
./gradlew :app:assembleDebug
```

The APK lands in `app/build/outputs/apk/debug/app-debug.apk`. CI builds the same
thing on every push as the `android (debug apk)` artifact, so a green run is the
easiest place to get one.

## Installing

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

The APK is a debug build signed with the debug key: fine for a phone you own,
not for distribution. It carries `arm64-v8a` and `x86_64`; an old 32-bit phone
will install and then fail to load the library.

## What it asks for

Four normal permissions, none of them prompted at runtime: `INTERNET` for the
listener, `ACCESS_NETWORK_STATE` and `ACCESS_WIFI_STATE` to see the network, and
`CHANGE_WIFI_MULTICAST_STATE` so the Wi-Fi stack stops filtering other devices'
announcements. No location permission, no storage permission: the identity and
received files live in the app's own `filesDir`.

Android hides multicast from apps unless a `MulticastLock` is held, so the app
takes one while the screen is alive. Without it the phone announces but never
hears anyone, which looks exactly like a broken receiver.

## The name and the fingerprint

On first run the phone picks two words from `flit-crypto`'s lists — `quiet
heron`, `slate otter` — and stores them in `name` beside `identity.key`. The
card's **Change** button edits that file; the receiver is restarted afterwards,
because the announce carries the name the engine started with. A name is a
label: it can be anything, including something another device already uses.

Under it the card shows the first 16 hex characters of the fingerprint. That is
the value the computer pins on the first send, and the only claim about identity
either screen can make.
