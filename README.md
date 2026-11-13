# kunianki

A Japanese country quiz built with Rust and Slint.

## Run on desktop

```sh
cargo run
```

Edit `ui/app-window.slint` to change the interface. Install `slint-lsp` and use the [Slint editor integration](https://github.com/slint-ui/slint/tree/master/tools/lsp) for formatting and language support.

On Linux, install `build-essential`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxkbcommon-dev`, `libfontconfig-dev`, `libinput10`, and `libgbm1`.

## Tests

Run unit and headless end-to-end tests with `cargo test`.

## Build for iOS

Install Xcode and [XcodeGen](https://github.com/yonaskolb/XcodeGen), then add the Rust targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd ios
xcodegen generate
open kunianki.xcodeproj
```

Build and run the generated project from Xcode.

For Intel-based simulator builds, also install the `x86_64-apple-ios` Rust target.

## Build for Android

Install Android Studio's Android SDK and NDK, then install [`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk) and the Rust target:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_ROOT="$ANDROID_HOME/ndk/<installed-version>"
gradle -p android :app:assembleDebug
```

Set `ANDROID_HOME` and `ANDROID_NDK_ROOT` to the SDK/NDK locations on your machine. The Gradle task builds the Rust library for `arm64-v8a` and packages it with the Android application.

## Mobile smoke checklist

- [ ] iOS simulator: run `xcodegen generate` in `ios/`, then build and launch from Xcode.
- [ ] Android emulator: run `gradle -p android :app:installDebug` from the repo root (or `./gradlew -p android :app:installDebug` if using a Gradle wrapper), then launch the app.
- [ ] Confirm the app launches and Japanese text renders correctly.
- [ ] Confirm flag and map questions display their images.
- [ ] Answer questions, restart the app, and confirm progress persists.
- [ ] Answer incorrectly and confirm the review count updates.
