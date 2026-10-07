# Tabular on Android (experimental)

This directory will hold the Android host project for Tabular. Today it only
contains this README: the Rust crate compiles for Android and produces
`libtabular.so`, but **the Gradle wrapper / activity project is not generated
yet**. You have to create it with Android Studio (or `gradle init`) and drop the
native library in, as described below.

## Prerequisites

- Rust toolchain with the Android target:
  `rustup target add aarch64-linux-android`
- [`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk): `cargo install cargo-ndk`
- Android NDK **r28** (tested with 28.2.13676358). Point `ANDROID_NDK_HOME` at it,
  or let `scripts/build_android.sh` find it under `$ANDROID_HOME/ndk/28.*`.
- Android SDK with platform 26 or newer and a JDK for Gradle.

## Build the native library

```bash
# quick compile check (what CI should run)
ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/28.2.13676358 cargo ndk -t arm64-v8a check

# release .so into android/app/src/main/jniLibs/arm64-v8a/libtabular.so
scripts/build_android.sh
```

The crate is built as a `cdylib`; the entrypoint is `android_main` in
`src/lib.rs`, driven by eframe's `android-native-activity` backend. Logs go to
logcat under the tag `tabular` (`adb logcat -s tabular RustStdoutStderr`).

## Host project (to be created)

Create an empty "No Activity" project in Android Studio with the application id
of your choice, module name `app`, minimum SDK 26. Then:

1. Make sure `android/app/src/main/jniLibs/arm64-v8a/libtabular.so` exists
   (output of `scripts/build_android.sh`). Gradle packages `jniLibs` automatically.
2. Replace the `<application>` block of `app/src/main/AndroidManifest.xml` with a
   `NativeActivity` entry. `android.app.lib_name` must be the crate name
   (`tabular`, i.e. `libtabular.so` without prefix/suffix) and `hasCode` must be
   `false` because there is no Java/Kotlin code:

   ```xml
   <application
       android:label="Tabular"
       android:hasCode="false"
       android:icon="@mipmap/ic_launcher">
       <activity
           android:name="android.app.NativeActivity"
           android:exported="true"
           android:configChanges="orientation|keyboardHidden|screenSize|uiMode"
           android:theme="@android:style/Theme.DeviceDefault.NoActionBar">
           <meta-data android:name="android.app.lib_name" android:value="tabular" />
           <intent-filter>
               <action android:name="android.intent.action.MAIN" />
               <category android:name="android.intent.category.LAUNCHER" />
           </intent-filter>
       </activity>
   </application>
   ```

   Add `<uses-permission android:name="android.permission.INTERNET" />` so the
   database drivers and sync can reach the network.
3. `./gradlew assembleDebug` and `adb install -r app/build/outputs/apk/debug/app-debug.apk`.

## Known gaps

- **No file picker yet.** `rfd` has no Android backend, so Open/Save dialogs,
  import/export to user-chosen files and the Data Files workspace browse buttons
  are compiled out (same as iOS). A Storage Access Framework bridge is still to do.
- **No external processes.** The `tabular mcp` CLI, external MCP clients,
  AI CLI backends, git client, `open`-style URL launching and desktop
  notifications are no-ops on Android and log `[ANDROID]` warnings instead.
- **Accessibility:** egui on NativeActivity cannot use AccessKit, so TalkBack
  does not see the UI (the `accesskit` feature is only available with
  GameActivity, which needs a Java/Kotlin host).
- **Keystore:** the `keyring` crate is unavailable; secrets use the existing
  encrypted-file store in the app data directory (`src/secrets.rs`).
- **No self-update:** updates come through the store/APK you install.
- Layout is tuned for desktop; the mobile device profile applies the same
  adjustments as iOS but has not been tested on real Android hardware.
