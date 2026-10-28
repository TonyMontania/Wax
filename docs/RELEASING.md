# Releasing Wax

Releases are made by hand from the maintainer's account. CI only builds.

## 1. Version

The version lives in one place: `version` in `Cargo.toml`. The in-app updater only offers a
release whose number is higher than the installed one, so every release needs a new number.
Add what changed to `CHANGELOG.md` (what users see, not internals).

## 2. Build

Run the **Build** workflow (Actions → Build → Run workflow). It makes, for each system, the files
below and opens each app for 15 seconds as a smoke test:

| Artifact | Files |
| --- | --- |
| `wax-Windows-x64` | `Wax_<v>_amd64-setup.exe`, `Wax-portable-<v>-win-amd64.zip` |
| `wax-Windows-arm64` | `Wax_<v>_arm64-setup.exe`, `Wax-portable-<v>-win-arm64.zip` |
| `wax-macOS-universal` | `Wax_<v>_universal.dmg`, `Wax_<v>_universal.app.tar.gz` |
| `wax-Linux-x64` | `Wax_<v>_amd64.AppImage`, `Wax_<v>_amd64.deb`, `Wax-<v>-1.amd64.rpm` |
| `wax-Linux-arm64` | `Wax_<v>_arm64.AppImage`, `Wax_<v>_arm64.deb`, `Wax-<v>-1.arm64.rpm` |

**Keep these names.** The processor is always `amd64` or `arm64`: GitHub lists a release's files
alphabetically and Wax 1.0.0's updater takes the first file that fits its kind of install
without looking at the processor, so the x64 file has to come first (`amd64` < `arm64`).

If Windows signing is set up (below), use the `…-signed` artifacts for Windows.

## 3. Sign the update files

The updater refuses files that aren't signed with the maintainer's key. The private key is
`~/.wax/wax-release.key` on the maintainer's computer — never in the repo, never in CI. **Keep a
backup** (a password manager or a USB stick): without it, installed copies can't update by
themselves anymore and people would have to download Wax again by hand.

```bash
cargo run --release --example sign_release -- sign ~/.wax/wax-release.key release/*
```

This writes `signatures.txt` (every file's signature, one per line) and a `.sig` next to each file.

- **1.2.0:** upload the files, `signatures.txt` **and the `.sig` files**. Wax 1.1.0 only knows
  the `.sig` way, and copies still on 1.1.0 must be able to update.
- **After 1.2.0:** upload only the files and `signatures.txt` (every copy that updates passes
  through a version that reads it). The release then lists each file once:
  `gh release create … $(ls release/* | grep -v '\.sig$')`

The matching public key is `UPDATE_KEY` in `src/updater.rs`. A new key pair
(`… -- keygen <file>`) means changing `UPDATE_KEY`, and copies with the old key won't accept
updates signed with the new one.

## 4. Publish

```bash
gh release create v<version> release/* --title "Wax <version>" --notes-file notes.md
```

Commits and releases carry only the maintainer's name.

## Windows code signing (SignPath)

Unsigned Windows builds show a SmartScreen warning. [SignPath Foundation](https://signpath.org)
signs open-source projects for free:

1. Apply at signpath.org with the repository link.
2. Once accepted, create in SignPath a project `wax`, a signing policy `release-signing` and an
   artifact configuration `windows` that signs the `*-setup.exe` and the `Wax.exe` inside the
   portable `.zip`.
3. In GitHub → Settings → Secrets and variables → Actions, add the variable
   `SIGNPATH_ORGANIZATION_ID` and the secret `SIGNPATH_API_TOKEN`.

From then on the Build workflow sends the Windows files to SignPath and keeps the signed ones as
`wax-Windows-x64-signed` and `wax-Windows-arm64-signed`.
