# Releasing

Two secrets have to exist before the first release. Without them the app still builds and
runs — it simply cannot update itself in place, and falls back to opening the release
page.

## The update signing key

The Windows updater verifies a minisign signature over the update manifest before
anything is written to disk. Certificate validation alone would not protect against a
compromised release bucket, which is the threat this exists for.

```bash
cd apps/studio
pnpm exec tauri signer generate -w ~/.tauri/master-design.key
```

Then:

1. Add `TAURI_SIGNING_PRIVATE_KEY` (the file's contents) and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to the repository's Actions secrets.
2. Add the **public** key to `src-tauri/tauri.conf.json`:

```json
"plugins": {
  "updater": {
    "pubkey": "<the public key>",
    "endpoints": [
      "https://github.com/kaiharimoto/Master-Design/releases/latest/download/latest.json"
    ]
  }
}
```

The public key is safe to commit. The private key is not, and the repository is
configured to refuse `*.key` files.

## The Android keystore

Android treats a differently-signed APK as a different app, so an unsigned build cannot
be installed as an update over a signed one.

```bash
keytool -genkey -v -keystore master-design.jks -keyalg RSA -keysize 2048 \
        -validity 10000 -alias master-design
base64 -w0 master-design.jks
```

Add as Actions secrets: `ANDROID_KEYSTORE` (the base64), `ANDROID_KEYSTORE_PASSWORD`,
`ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`.

**Keep this file.** Losing it means no existing installation can ever be updated again;
every user has to uninstall and reinstall.

## Cutting a release

```bash
git tag v0.2.0
git push origin v0.2.0
```

The workflow builds Windows and Android, collects the artifacts, writes a checksum table
into the release body, generates `latest.json`, and publishes.

The checksum table is not decoration: the Android updater reads it to verify the APK it
downloads. A checksum in a separate file is a checksum that can go missing, so it lives
in the release body next to the artifact it describes.

## Verifying an update actually works

Signing is the kind of thing that appears to work until the first real update fails.
Before announcing a release:

1. Install the *previous* version.
2. Publish the new one.
3. Confirm the banner appears, the update applies, and the app restarts on the new
   version — on both platforms.

On Android, also confirm the install intent is accepted; some OEM builds refuse it, and
the fallback to the release page should be what a user sees rather than an error.
