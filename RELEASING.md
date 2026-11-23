# Releasing Gyroflow (Omni)

Omni has no CI. Each release is built by hand: `Gyroflow-windows64.zip` on a Windows PC and `Gyroflow-mac-universal.dmg` on a Mac. Both go into one **draft** GitHub release, which you publish once it has both files.

Omni has its own version line, starting at 1.0.0, separate from upstream Gyroflow. The app's update check only looks at the latest published release of [`yshui1/gyroflow-omni`](https://github.com/yshui1/gyroflow-omni/releases) and never sees drafts or prereleases. Upstream Gyroflow releases and announcements are not shown.

The plugins are released the same way, from `gyroflow-plugins-omni`. See `RELEASING.md` in that repo. Release the plugins first if the new app version depends on them.

## One-time setup

### Windows

- The normal build dependencies (`just install-deps`), LLVM in `C:\Program Files\LLVM`, 7-Zip and the [GitHub CLI](https://cli.github.com) (`gh auth login`)
- `just`: `winget install Casey.Just`

### Mac

- The normal build dependencies (`just install-deps`, which also fetches the Intel and Apple Silicon FFmpeg and OpenCV for the universal build), `gh` (`gh auth login`) and `just` (`brew install just`)
- Signing and notarization, so macOS opens the app without warnings:
  1. Install your **Developer ID Application** certificate in Keychain. It's under Certificates in your Apple Developer account.
  2. The scripts use it automatically when it's the only Developer ID Application certificate in Keychain. If you have several, find the hash (`security find-identity -v -p codesigning`) and run the script with `SIGNING_FINGERPRINT=<hash>`.
  3. Save your notarization login. It asks for an app-specific password, which you create at [account.apple.com](https://account.apple.com):

     ```bash
     xcrun notarytool store-credentials omni-notary --apple-id <your Apple ID> --team-id <your team ID>
     ```

     The profile stays in your login Keychain, so every later release (of the app and the plugins) reuses it. You only redo this on a new Mac or after revoking the app-specific password. To check it, run `xcrun notarytool history --keychain-profile omni-notary`: it should print "Successfully received submission history". The scripts run the same check before building. To use a profile with another name, set `NOTARY_PROFILE=<name>`.

## Releasing a version

### 1. Set the version

The app version is in each of these files. The core crate (`src/core/Cargo.toml`) stays at upstream's version, because it is written into project files and lens profiles.

- `Cargo.toml` (`[package] version`)
- `_scripts/common.just` (`ProjectVersion`)
- `src/cli.rs` (the `Gyroflow (Omni) v…` doc line)
- `_deployment/mac/Gyroflow.app/Contents/Info.plist` and `_deployment/ios/Info.plist`
- `_deployment/android/AndroidManifest.xml`
- `_deployment/windows/AppxManifest.xml` (`<version>.0`)

Commit and push. The release is tagged at the commit you're on, so it has to be on GitHub.

Write the release notes to a Markdown file, e.g. `notes.md`. The app shows them in its "There's a newer version available" popup. Without `-NotesFile` / `--notes`, GitHub generates notes from the commit list.

### 2. Build on Windows

```powershell
.\_scripts\omni-release.ps1 -NotesFile notes.md
```

This runs `just deploy` with `OMNI_RELEASE=1` and checks that `gyroflow.exe --version` prints `Gyroflow (Omni) v<version>`. It then creates the draft `v<version>` and uploads `Gyroflow-windows64.zip`.

### 3. Build on the Mac

Pull the same commit, then:

```bash
./_scripts/omni-release.sh
```

This runs `just deploy universal` (Intel + Apple Silicon) with `OMNI_RELEASE=1` and signs the app and dmg with your Developer ID. It then notarizes and staples the dmg, checks it with Gatekeeper and uploads it to the same draft. Notarization usually takes a few minutes. If Apple rejects it, the script prints the submission and the command that shows Apple's log.

Either OS can go first. Whichever runs first creates the draft, and the other adds to it. Running a script again replaces its file in the draft.

### 4. Check and publish

Download both files from the draft and launch them. On the Mac, the app should open with no Gatekeeper prompt. Then run either of these:

```powershell
.\_scripts\omni-release.ps1 -Publish
```

```bash
./_scripts/omni-release.sh --publish
```

Publishing checks that both files are in the draft, asks you to confirm, then publishes the release as the latest. Running copies of older versions show the update popup on their next start.

## Script options

| Windows | Mac | Does |
| --- | --- | --- |
| `-NotesFile <file>` | `--notes <file>` | Release notes (also replaces the notes of an existing draft) |
| `-DryRun` | `--dry-run` | Build and check, but don't touch GitHub. On the Mac it still notarizes. |
| `-SkipBuild` | `--skip-build` | Upload the file already in `_deployment/_binaries` |
| `-Publish` | `--publish` | Publish the draft instead of building |
| | `--adhoc` | Allow a build without a Developer ID (test builds only; users must allow it in Privacy & Security on first launch) |

## Version numbers

- `OMNI_RELEASE=1` (set by the scripts) makes the app report plain `1.0.0`.
- Other local builds report `1.0.0` too, without a dev suffix, unless built by GitHub Actions (`1.0.0 (gh…)`). Both kinds of build check for updates and install plugins from GitHub releases.
- `just publish <version>` (inherited from upstream) bumps the version files above in a fresh clone, but it also runs `cargo update` on every dependency, then commits, tags and pushes to `master`. Edit the versions by hand unless you want all of that.

## Troubleshooting

- **"isn't pushed to origin" / "uncommitted changes":** commit and push first. Untracked files are ignored.
- **"is already published":** published releases are never overwritten. Bump the version for a fix.
- **`gyroflow.exe` reports the wrong version:** a stale build was reused. Run again without `-SkipBuild`.
- **The GitHub Actions workflow** (`.github/workflows/release.yml`) is upstream's and only runs when started by hand. It needs upstream's store and signing secrets, so it isn't part of this process.
