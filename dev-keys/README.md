# Development signing keys

> **This private key is committed on purpose, and it must be replaced before the first
> public release.** It exists so the update channel can be built and tested end to end
> without a human having to generate anything first. Anyone with this file can sign an
> update that Master Design will accept as genuine.

## What is here

| File | What it is |
| --- | --- |
| `updater.key` | minisign private key, **no passphrase** |
| `updater.key.pub` | the matching public key, also pasted into `apps/studio/src-tauri/tauri.conf.json` |

The passphrase is deliberately empty rather than committed alongside the key: a
passphrase stored next to the thing it protects is theatre, and pretending otherwise
would make the setup look safer than it is.

## Why commit a private key at all

The desktop updater verifies a minisign signature over the update manifest before it
writes anything to disk. Without a keypair there is no signature, so the whole update
path — build, sign, publish, detect, download, verify, install — cannot be exercised at
all. Testing that path is worth more right now than the secrecy of a key that protects
nothing anyone has installed yet.

## Rotating before release

Do this before publishing anything anyone else will install.

```bash
cd apps/studio
pnpm exec tauri signer generate -w ~/.tauri/master-design.key   # choose a real passphrase
```

Then:

1. Put the **public** key in `apps/studio/src-tauri/tauri.conf.json` under
   `plugins.updater.pubkey`.
2. Put the **private** key and its passphrase in the repository's Actions secrets as
   `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Delete this directory and remove the `!dev-keys/*.key` exception from `.gitignore`.
4. Cut a release and confirm an older build actually accepts the update.

Note that rotating the key means installations signed with the old one will refuse the
new update — every existing install has to be replaced manually. That is only acceptable
while the only installations are yours, which is exactly why this is a pre-release step
rather than something to defer.
