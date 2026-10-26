# Changelog

## 2026-10-26

### Stage 3: Work Persistence and Back-Button Navigation

- Added smart debounced autosaving after 3 seconds from the last document edit, immediately on `visibilitychange` (hidden), `pause` lifecycle event, and before app exit (`pagehide`/`beforeunload`).
- Implemented persistent atomic document storage via `@capacitor/filesystem` (Data directory) using a temporary file and automatic `.bak` backup copy.
- Added automatic recovery of the active document on app launch following an abnormal exit/crash with an in-app confirmation message ("تم استرجاع عملك"), falling back to `.bak` if the primary copy is corrupted.
- Created the Documents Management screen (`AppScreen::Documents`) displaying document titles, modification timestamps, and file sizes, with actions to create new documents, open existing documents, rename, delete with confirmation, and share/export DOCX and PDF via `@capacitor/share`.
- Integrated Android back-button handling via `@capacitor/app`: dismisses open modal dialogs, popups, and menus first; from the document editor, saves and navigates back to the documents screen; from the documents screen, exits the app cleanly.
- Added comprehensive Rust unit tests covering atomic file saving, backup creation, resilient restoration from `.bak` upon corruption, and graceful error reporting for damaged files.
- Fixed Gradle wrapper permissions in the Git index (`100755`) and added automated GitHub Release creation (`softprops/action-gh-release`) on pushes to `main`.

## 2026-10-25

### Stage 2

- Added the missing web app icon in PNG and multi-resolution ICO formats, using the Android
  launcher artwork so Trunk can resolve the icon referenced by `apps/wordcraft-web/index.html`.
- Added a manually triggered keystore-generation workflow that uploads a short-lived,
  access-controlled setup artifact; signing values are masked from workflow logs.
- Added optional release signing from GitHub Actions secrets, with a debug-signing fallback when
  release secrets are absent, and a separate `Ard-release.apk` artifact when configured. Release
  version codes increase with each new workflow run.
- Added tablet-focused release setup instructions in `docs/RELEASE.md` and ignored local keystore
  and signing-secret files.
- Not verified: a real release build and installation/update over the existing debug APK require
  the user-generated keystore and configured GitHub Actions secrets.

### Stage 1

- Added a GitHub Actions workflow to build the WASM web app and upload the Android debug APK as
  `Ard-debug` on pushes to `main` or manual dispatch.
- Renamed the Android application to أرض and changed its application ID and namespace to
  `com.zulfiqar.ard`; moved the Java activity and test packages to match.
- Updated the web page language, direction, title, and loading message for Arabic branding.
- Added the requested in-app attribution, author information, and modifications attribution while
  preserving the original license files and attribution records.
- Added the contributor display name and contributor metadata for Zulfiqar Alhmood.
- Fixed the contributor generator's commit-record parser so it can produce the required credits
  JSON from this repository's Git history.
