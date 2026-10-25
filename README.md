# أرض

**المطوّر:** Zulfiqar Alhmood

أرض تطبيق لمعالجة المستندات، مبني على WordCraft ومهيأ للويب وAndroid.

## الأصل والإسناد

أرض مبني على [WordCraft](https://github.com/storytold/wordcraft)، المرخّص بموجب MIT وApache-2.0.
تظل إشعارات التراخيص والإسناد الأصلية محفوظة في `LICENSE-MIT` و`LICENSE-APACHE` و`NOTICE` و
`ATTRIBUTION.md`.

## About the upstream source

This is a trimmed copy of WordCraft (<https://github.com/storytold/wordcraft>, MIT OR
Apache-2.0) containing **only** what `apps/wordcraft-web` (and, through it, the Android app in
`apps/wordcraft-android`) needs to build. Removed, on purpose: the desktop app, the CLI, the MCP
server, the desktop TCP control channel, `xtask`, and all non-web packaging — none of those are
on the dependency path of the web/Android build (verified by grepping every kept `Cargo.toml`).

Kept crates: `geom, doc, fonts, layout, docx, docbin, formats, render, pdf, proof, metafile,
zotero, engine, ui-egui` — exactly the transitive dependency set of `wordcraft-ui-egui`, which
`apps/wordcraft-web` runs.

## Build

```sh
# 1. Install Capacitor assets needed by the web build
cd apps/wordcraft-android
npm install

# 2. Build the WASM web app (output: dist/web relative to repo root)
cd ../wordcraft-web
trunk build --release

# 3. Sync into the Android (Capacitor) shell and build the APK
cd ../wordcraft-android
npm run android:sync         # cap sync android
npm run android:apk          # ./gradlew assembleDebug  (output under android/app/build/outputs/apk)
```

You need: Rust with `wasm32-unknown-unknown`, [`trunk`](https://trunkrs.dev), Node.js, and an
Android SDK (API 34) + JDK 17 for Gradle.

## Build APK with GitHub Actions

The `Android APK` workflow runs on pushes to `main` and can also be started manually with
`workflow_dispatch`. In the repository's **Actions** tab, choose **Android APK**, select
**Run workflow**, and confirm the `main` branch. When the run succeeds, open it, download the
`Ard-debug` artifact, and extract the ZIP to get the APK.

For signing a release APK so future updates can be installed over it, follow the tablet-friendly
one-time setup in [docs/RELEASE.md](docs/RELEASE.md). Keep the generated signing key private and
back it up securely; without its original key, future APKs cannot update that installation.

## Fixes applied versus the upstream file set

- **Missing `colors.xml` (real build breaker).** `android/app/src/main/res/values/styles.xml`
  references `@color/colorPrimary`, `colorPrimaryDark`, `colorAccent`, but no `colors.xml` shipped
  them — `./gradlew assembleDebug` would fail with "resource color/colorPrimary not found". Added
  `android/app/src/main/res/values/colors.xml` with neutral placeholder colours; swap in your own
  brand colours whenever you have them.
- **`minSdkVersion` 22 → 26.** Android 5.1 is a negligible share by 2026 and ships a WebView too old
  for reliable WebGL2 (the `eframe`/`wgpu` fallback path this app depends on); 26 (Android 8) is a
  safer modern floor.
- **Release build wasn't shrunk.** `minifyEnabled` was `false` and resources weren't shrunk, so
  release APKs were needlessly large. Turned on `minifyEnabled true` + `shrinkResources true`, and
  added the R8/ProGuard keep rules Capacitor needs (its plugins — Filesystem, Preferences, Share —
  are found by reflection, which R8 can't see from the call graph; without explicit `-keep` rules
  they'd be stripped or renamed and the JS-to-native bridge calls would silently fail at runtime).
- **Keyboard could cover the canvas on some devices.** `android:windowSoftInputMode` was left
  unset (OEM-dependent default). Set it explicitly to `adjustResize` so the activity's window
  actually shrinks for the keyboard, matching the `100dvh` / `interactive-widget=resizes-content`
  behaviour `index.html` already assumes.
- **Android package name.** The activity and test sources now use `com.zulfiqar.ard`, matching
  both the Gradle namespace and `applicationId`; the manifest names the activity explicitly.

## Still worth doing (not mechanical — real engine/UX work, left for you to scope)

- **Touch text selection handles.** egui has no Android-style drag handles for adjusting a
  selection; editing by touch currently relies on drag-select only.
  `crates/ui-egui/src/canvas.rs` is where this would live.
- **Long-press context menu** (copy/paste/format) as an alternative to keyboard shortcuts, since
  there's no physical keyboard on most Android devices.
- **Back-button handling**: `MainActivity` is still the stock
  `class MainActivity extends BridgeActivity {}`; Capacitor's default back-button behaviour (exit
  or WebView-back) doesn't know about unsaved WordCraft documents. Hooking into the `App` Capacitor
  plugin's `backButton` listener and WordCraft's own `recovery.md`/autosave logic would need
  wiring on both the Rust (`web.rs`) and Android side.
- **Custom launcher icon / splash** — still the generic Capacitor-template assets under
  `res/mipmap-*` and `res/drawable*/splash.png`.

## Contributors

`contributors/contributors.json` supplies commit statistics to the in-app contributors view;
`contributors/people.toml` supplies verified display names. Zulfiqar Alhmood is listed for the
Android and Arabic app modifications in this repository.

Per the MIT/Apache-2.0 licence this is built on, keep `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`
and `ATTRIBUTION.md` in place — they record the original project's copyright and the licences of
every bundled asset (fonts, hyphenation dictionaries, etc.). Crediting yourself for your own
Android port and Arabic-localization work is fine and encouraged; presenting the ~240k-line engine
underneath as entirely your own original work would not be accurate.
