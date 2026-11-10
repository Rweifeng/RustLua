# Android plugin

This NativeActivity template is used by `scripts/build/package-android.ps1`. The publisher stages
the selected project's packaged resources in the APK assets and builds the Rust runtime as
`libgame.so`; the Android entry point extracts the bundled project into app-private storage.

The current target is a sideloadable `arm64-v8a` APK. Its release variant is signed with the
local Android debug key for installation/testing; it is not a Play Store signing configuration.

Required tools (the publishing panel can install them to
`%LOCALAPPDATA%\RustLua\android-toolchain` and stores custom paths in the user-level
`~/.rustlua/editor.json`):

- JDK 17
- Android SDK platform 35 and a compatible Android NDK
- Gradle 8.9 or newer (the Android Gradle plugin is pinned to 8.7.3)
- Rust target `aarch64-linux-android`
- `cargo-ndk`

The installer pins Android SDK platform 35, build-tools 35.0.0, NDK 27.2.12479018, Gradle 8.9,
and Temurin JDK 17. It verifies the vendor archive checksums before extraction. It also installs
the Rust `aarch64-linux-android` target and `cargo-ndk`. The user must explicitly accept the
Android SDK licenses in the publishing panel before the installer submits that acceptance.

The publishing panel passes the configured paths directly to the build script; it does not modify
global environment variables. The application ID is supplied by the editor or the script's
`-ApplicationId` parameter.
