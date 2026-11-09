# Nikon RAW desktop workflow

## Changes

- Core metadata loading accepts NRAW `.NEV` and Nikon R3D NE `.R3D`, including uppercase extensions and NRAW files already renamed to `.R3D`. It reads the container instead of guessing timing or image size.
- Preview and analysis use the existing MDK RED reader. GPU decoding can be disabled consistently for both paths.
- Lens calibration accepts `.NEV`.
- Multiple Nikon `.R3D` files open the usual batch-import choices. Only non-Nikon RED recordings retain the first-segment shortcut.
- Both `.NEV` and `.R3D` show the REDline conversion notice for desktop export. RAW conversion controls are labeled for RAW footage.
- Missing REDline, a failed child process, or a missing converted output stops export. A failure no longer falls through to FFmpeg with an unsupported RAW input.

Internal decoder names, SDK installer URLs, and saved settings keys remain compatible.

## Decoder and export requirements

The existing desktop installers bundle RED SDK 9.2.1. Its release notes include Nikon N-RAW and R3D NE fixes; see the [official RED SDK release history](https://www.reddigitalcinema.com/download/r3d-sdk). Preview/analysis use the [MDK RED reader](https://github.com/wang-bin/mdk-r3d).

Desktop export uses the existing REDline-to-ProRes workflow, followed by stabilization and encoding. Install a REDCINE-X PRO / REDline release supporting the Nikon recording format. These changes do not add a RAW encoder. Alternatively, export a `.gyroflow` project including gyro data and use a video editor plugin to retain the editor's RAW development controls.

## Camera sample validation

| File | Dimensions | Frame rate | Frames | Duration | Extracted IMU samples |
| --- | --- | --- | --- | --- | --- |
| DSC_0073.NEV | 6060x3410 | 24000/1001 | 147 | 6.131125 s | 0 |
| DSC_0073.R3D (renamed NRAW) | 6060x3410 | 24000/1001 | 147 | 6.131125 s | 0 |
| DSC_0003.R3D (native R3D NE) | 6060x3410 | 24000/1001 | 15 | 0.625625 s | 0 |

These are metadata/container results, not decoded-frame or full desktop export results. No motion samples are recovered by the current telemetry parser from these files. Use a suitable lens profile and synchronized external motion data for gyro stabilization.

## Repeatable checks

Run from the desktop repository in PowerShell:

```powershell
$env:GYROFLOW_NIKON_SAMPLE_DIR = 'C:\Raw Video\Gyroflow NRAW'
cargo test --manifest-path src/core/Cargo.toml --locked --no-default-features --lib util::tests -- --include-ignored --nocapture
cargo test --manifest-path src/core/Cargo.toml --locked --no-default-features --lib raw_video::tests -- --include-ignored --nocapture
cargo test --manifest-path src/core/Cargo.toml --locked --no-default-features --test desktop_red_sdk -- --nocapture
```

The synthetic fixture tests container timing with NRAW/NR3D sample-entry tags and rejects invalid input. The camera tests use the originals without rewriting them. The desktop REDline adapter is compiled through a core integration-test harness, independently of Qt.

## Windows build

The app display name is **Gryoflow (Omni)**. The executable metadata, GUI title, CLI version text, and Windows/macOS package display labels use this name. Internal application identifiers and the settings path stay compatible with the existing checkout.

A Windows debug build succeeded with Qt 6.7.3, FFmpeg 9.0, OpenCV 4.14.0, OpenCL, MDK 0.39.0, and RED SDK 9.2.1. Build dependencies are under the ignored `ext/` directory. The build has OpenCV support enabled and optional AI optical flow disabled.

To rebuild with the provisioned dependencies:

```powershell
.\_scripts\build_windows_omni.ps1 -NoAiOpticalFlow
```

Add `-Release` for an optimized build. Omitting `-NoAiOpticalFlow` enables the default AI feature and requires a separate full-feature build verification.

The local runnable debug folder is `target/Gryoflow-Omni/`, with `Gryoflow (Omni).exe`, matching debug Qt plugins/runtime, FFmpeg/OpenCV/MDK/RED DLLs, and lens profiles. This is a local test build, not a release installer. Keep the folder together.

Verified:

- Full desktop compilation and linking completed. This caught and fixed the QUrl-to-encoded-string conversion in Nikon batch detection.
- The packaged executable's `--version` command exited successfully and reported `Gryoflow (Omni) v1.6.3`.
- Windows executable ProductName and FileDescription show the new name.
- A hidden GUI startup with `DSC_0003.R3D` remained running and reached the Nikon decoder setup. The test process was then closed; this is not visual playback verification.
- A native MDK/RED SDK first-frame test decoded each of the three original samples on CPU, producing a reduced-resolution 378x212 frame. The test harness and binaries are under ignored `target/`.
- Core metadata, RAW routing/container tests, and the desktop REDline adapter test passed (9 tests).

Remaining manual checks:

1. Inspect visual preview and seeking for each sample, with GPU decoding enabled and disabled.
2. Import multiple native Nikon `.R3D` clips and confirm batch choices appear.
3. Load an external gyro source, synchronize it, and test stabilization with a suitable lens profile.
4. Verify the full REDline-to-ProRes-to-stabilized-export workflow with a compatible REDline installation.

## Black stabilized preview regression

The Nikon ZR manual-lens sample records 0.0 mm as an unknown focal length. Accepting it as projection data replaced the fallback camera matrix with a singular matrix and produced a black stabilized preview (about 0.3% zoom). The decoder itself produced valid non-black RGB pixels on both CPU and GPU.

The metadata reader now ignores non-positive/non-finite focal lengths, and the projection path also validates saved/imported focal values before replacing the fallback matrix. The regression failed with zero focal entries before the fix and passes afterward. The rebuilt GUI showed DSC_0073.R3D with stabilization enabled at about 100% zoom.

The verified runnable candidate is target/Gryoflow-Omni-Fixed/Gryoflow (Omni).exe. It uses the current MDK 0.39.0 nightly runtime. A separate QR overlay remains: the bundled upstream MDK license key reports that it does not cover the renamed app. This is distinct from the corrected projection failure.

## Omni MDK setup and playback measurements

Omni no longer submits the upstream application's name-bound MDK key. It uses the current SDK's documented keyless mode by default. An app-specific key issued by the MDK author can be supplied through the GYROFLOW_OMNI_MDK_KEY environment variable; the key is not checked into this repository. See the vendor's licensing policy: https://github.com/wang-bin/mdk-sdk/wiki#license . Keyless deployments need a current SDK; long-term pinned releases should use a key issued for Omni.

The NEV and renamed R3D samples have the same SHA-256. Using MDK 0.39.0 nightly with RED SDK 9.2.1 and scale=1920x1080, the 147-frame samples measured:

| Decoder | NEV | Renamed R3D |
| --- | --- | --- |
| GPU auto | 105.4 fps | 107.1 fps |
| CPU | 19.4 fps | 19.7 fps |

These are native decoding throughput measurements, not end-to-end preview frame rates. There was no reproduced extension-specific decoder penalty. The user confirmed smooth playback with the updated keyless candidate. Earlier reports that the QR overlay remained refer to builds still passing the incompatible upstream key.

An optimized Windows build with OpenCV and without optional AI optical flow also succeeded. Its runnable folder is target/Gryoflow-Omni-Release; its version-command smoke test exited successfully. It has not been visually tested because the user stopped Computer Use with Escape after confirming smooth playback in the keyless candidate.

## Issued Omni application key

The MDK maintainer supplied an app-specific key for Gryoflow (Omni) on October 31, 2026. The local Windows build stores it in ignored `ext/mdk-license-key.txt`. `_scripts/build_windows_omni.ps1` loads that file into the build environment, and the app embeds it through `option_env!("GYROFLOW_OMNI_MDK_KEY")`.

The optimized app can therefore be launched normally without a key environment variable. A non-empty runtime `GYROFLOW_OMNI_MDK_KEY` still overrides the embedded key. Public clones without local key configuration retain keyless mode. The key itself is absent from tracked source files.

The maintainer's reply did not explicitly state expiration or SDK-pinning terms. Do not infer additional terms from the successful key integration.

Verification: the rebuilt optimized executable contains the issued key and launched with the runtime key environment variable removed. The application reached MDK license configuration and Nikon R3D decoding without an invalid-key, app-name-mismatch, or outdated-SDK message. The actual GUI displayed DSC_0073.R3D with stabilization enabled and no MDK QR overlay. The licensed runnable build is target/Gryoflow-Omni-Release/Gryoflow (Omni).exe.
