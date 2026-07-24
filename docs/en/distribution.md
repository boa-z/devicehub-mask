# CI, Releases, and Updates

[简体中文](../zh-CN/distribution.md) | [Documentation](README.md)

## Workflow Triggers

`.github/workflows/nightly.yml` runs on commits and manual dispatches only. It has no scheduled trigger, does not use GitHub Environments, and therefore does not create Deployment records that obstruct history cleanup.

## Jobs

- **verify** is a fail-independent macOS, Windows, and Linux matrix. Each leg runs frontend lint, tests, and build; Rust format, tests, and Clippy; and a debug Tauri application build.
- **build-macos** creates a Universal Apple Silicon/Intel DMG and verifies both executable architectures and the complete application signature.
- **build-windows** creates x64 NSIS and MSI installers.
- **build-linux** creates x64 AppImage and DEB packages.
- **publish-nightly** waits for every package, merges updater fragments into one `latest.json`, and atomically replaces the rolling nightly release assets.

Workflow artifacts are retained for 14 days. The rolling public release is:

<https://github.com/boa-z/devicehub-mask/releases/tag/nightly>

## Versions and Artifacts

Installer filenames contain the base version and workflow build number. The run number becomes `CFBundleVersion` on macOS. Updater artifacts use a shared `major.minor.<run-number>` version because SemVer build metadata such as `0.1.0+12` does not affect update ordering.

The release can contain:

```text
devicehub-mask_<base-version>+<build>_universal.dmg
devicehub-mask_<base-version>+<build>_universal.dmg.sha256
devicehub-mask_<base-version>-<build>_universal.app.tar.gz
devicehub-mask_<base-version>-<build>_universal.app.tar.gz.sig
devicehub-mask_<base-version>+<build>_x64-setup.exe
devicehub-mask_<base-version>+<build>_x64-setup.exe.sig
devicehub-mask_<base-version>+<build>_x64.msi
devicehub-mask_<base-version>+<build>_amd64.AppImage
devicehub-mask_<base-version>+<build>_amd64.AppImage.sig
devicehub-mask_<base-version>+<build>_amd64.deb
latest.json
```

## Tauri Updater Signing

Updater signatures are independent of Apple code signing. The public key is committed in `src-tauri/tauri.conf.json`; the private key must never be committed.

Generate a replacement keypair only before publishing the first compatible release:

```sh
mkdir -p .tauri
npm run tauri -- signer generate --write-keys .tauri/devicehub-mask.key
```

Update `plugins.updater.pubkey`, then configure repository Actions secrets:

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Complete private key file contents |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Generation password, or empty |

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < .tauri/devicehub-mask.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

Without the private key, CI can still publish native installers but skips updater signatures and `latest.json`. Losing or replacing the key prevents existing installations from accepting future updates.

At runtime, automatic nightly checks can be disabled in Settings. The setting is stored as `devicehub-mask.updates.automatic`; the manual check remains available. Accepted updates are downloaded, verified, installed, and followed by restart.

## Apple Signing and Notarization

Current nightly macOS apps receive a structurally valid ad-hoc signature after Universal assembly and version stamping. This verifies sealed resources and binary slices but does not establish publisher identity. Gatekeeper may require explicit approval.

Production distribution should configure a Developer ID Application certificate, notarize the DMG, and staple the notarization ticket. Apple signing does not replace the Tauri updater signature.

## Release Checklist

1. Run the validation commands in [Development](development.md).
2. Confirm the commit author and target branch.
3. Push `main` or manually dispatch the workflow.
4. Verify all three matrix jobs and all package jobs.
5. Confirm the release contains the expected native packages, signatures, and `latest.json`.
6. Install at least one produced package rather than testing only a Cargo target executable.
