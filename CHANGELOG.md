# Changelog

## 2026-10-25

### Stage 2

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
