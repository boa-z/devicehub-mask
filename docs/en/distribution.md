# CI, Releases, and Updates

[简体中文](../zh-CN/distribution.md) | [Documentation](README.md)

## Workflow Triggers

`.github/workflows/nightly.yml` runs on commits and manual dispatches only. It has no scheduled trigger, does not use GitHub Environments, and therefore does not create Deployment records that obstruct history cleanup.

`.github/workflows/cleanup-nightly.yml` runs weekly and can be dispatched manually. It retains the newest 20 completed nightly workflow runs and deletes nightly artifacts older than 14 days by default. Manual runs can change both bounded retention values or use dry-run. It never deletes the rolling `nightly` release, tag, or release assets.

`.github/workflows/release.yml` is manual-only. Select the Git ref containing the exact source to release, enter the tag matching `v<tauri.conf.json version>`, and choose whether to retain a draft. It reuses the same verification and packaging workflow as Nightly, but injects the Stable channel and the plain product version. Stable tags and published releases are immutable. The workflow creates or resumes a draft, uploads all assets, and only then publishes it as the repository's latest release when **Draft** is disabled.

## Jobs

- **verify** is a fail-independent macOS, Windows, and Linux matrix. Each leg runs frontend lint, tests, and build; Rust format, tests, and Clippy; and a debug Tauri application build.
- **build-macos** creates a Universal Apple Silicon/Intel DMG and verifies both executable architectures and the complete application signature.
- **build-windows** creates x64 NSIS and MSI installers.
- **build-linux** creates x64 AppImage and DEB packages.
- **publish-nightly** waits for every package, merges updater fragments into one `latest.json`, and atomically replaces the rolling nightly release assets.

Workflow artifacts are retained for 14 days. The rolling public release is:

<https://github.com/boa-z/devicehub-mask/releases/tag/nightly>

## Versions and Artifacts

`tauri.conf.json` contains the target stable product version. A Nightly build derives the standard SemVer prerelease `<product-version>-nightly.<run-number>`; for example, build 96 targeting version `0.1.0` is `0.1.0-nightly.96`. Numeric prerelease identifiers order Nightly builds, and the final `0.1.0` release sorts above every `0.1.0-nightly.*` build. After publishing a stable release, increment the product version before producing further Nightly builds.

Installer filenames contain the product version and workflow build number. The run number also becomes `CFBundleVersion` on macOS. Settings reports **Version**, **Build**, and the seven-character **Commit** separately; the selected update channel identifies a Stable or Nightly build without exposing an internal second version.

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

At runtime, Settings can select the **Stable** or **Nightly** update channel and disable automatic checks. The preferences are stored as `devicehub-mask.updates.channel` and `devicehub-mask.updates.automatic`; the manual check remains available. Stable checks use `releases/latest/download/latest.json`, while nightly checks use the rolling `releases/download/nightly/latest.json`. Both routes accept the same signed Tauri manifest format. Accepted updates are downloaded, verified, installed, and followed by restart. Until a stable release publishes a signed `latest.json`, checking Stable reports the missing manifest instead of silently falling back to Nightly.

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

## Stable Release Procedure

1. Update `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `package.json` to the intended stable version, keeping the three values equal.
2. Complete local verification and push the exact release commit.
3. Open **Actions > Publish Stable Release**, select that Git ref, and enter `v<version>` (for example, `v0.1.0`).
4. Keep **Draft** enabled for a release candidate inspection, or disable it to publish automatically after every package has uploaded.
5. Verify clean installation and in-app Stable update on macOS, Windows, and Linux.
6. After publication, increment the configured product version before the next Nightly build.
