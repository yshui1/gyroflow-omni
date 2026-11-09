# Lens calibration with log footage

In **Lens calibrator → Calibration → Calibration LUT**, choose:

- **Auto detect** (default): select a built-in LUT when camera brand, model and
  explicit gamma metadata identify a supported transform. Missing, unknown or
  unsupported gamma metadata leaves the LUT off and displays a message.
- **None**: analyze the decoded footage without a color transform.
- A built-in camera/profile LUT: manually override detection.
- **Browse LUT…** or **Custom LUT…**: load a 3D `.cube` file.

The nine bundled choices cover DJI Osmo D-Log M, DJI Pocket 4P D-Log and D-Log2,
GoPro Mission 1 GP-Log2, Nikon Log3G10 and N-Log, Panasonic LogC3 and V-Log, and
Sony S-Log3. ETTR variants are not included.

The LUT transforms decoded RGB before grayscale pattern detection and sharpness
measurement, for both Auto calibrate and manually added calibration points.
Supported input footage can be calibrated directly without exporting an
intermediate Rec.709 video. This uses the application's existing decoders; it
does not add RAW formats or change RAW development settings. Match the LUT to
the decoder's actual gamma and gamut, rather than the file extension.

The transform affects calibration analysis and the calibrator's GPU preview.
It uses the existing Qt RHI render pass with two filtered texture samples per
pixel; the half-float LUT atlas is uploaded only on selection. Paused frames
refresh immediately, the LUT remains visible with stabilization disabled, and
calibration overlays keep their original colors. Selecting None restores the
original preview shader. Active LUT preview uses Qt RHI even when an alternative
processing pipeline was selected; software-only preview does not apply the LUT.
Ordinary stabilization/export rendering is unchanged, and the LUT is not baked
into the lens profile. Changing the LUT clears detected patterns, forced points and the
previous fit; run calibration again before exporting a profile. Manual choices
are reused for subsequent clips in the same calibrator window. Auto detection
is evaluated separately for each clip.

Custom LUTs support `LUT_3D_SIZE` from 2 to 129, optional `TITLE`, comments,
`DOMAIN_MIN` and `DOMAIN_MAX`, with trilinear interpolation and clamped inputs.
1D/combined LUTs, malformed entries and non-finite numbers are rejected with an
error while preserving the current LUT. Reopening a clip after its custom LUT
has been removed displays the load error and selects None.

## Validation

The standalone LUT tests can run without Qt, FFmpeg or OpenCV:

```powershell
rustc --edition 2024 --test src/calibration_lut.rs -o target/calibration-lut-tests.exe
& target/calibration-lut-tests.exe
```

They cover red-fastest ordering, trilinear interpolation, input domains,
clamping, invalid LUTs, RGB48 row stride, luma conversion, conservative metadata
matching and parsing every bundled LUT.

The Windows GPU check in `src/qt_gpu/preview_lut_test.pro` builds with Qt's qmake
and an MSVC developer shell. Regenerate shaders with
`_scripts/compile_preview_shaders.ps1`. Compile `preview_lut_baseline.frag` using
qsb, then pass the texture vertex QSB, fisheye LUT fragment QSB and baseline QSB
to `preview-lut-test.exe`. The offscreen D3D11 test checks every output pixel at
4K and compares GPU timestamps across 200 interleaved frames after warmup.

On an NVIDIA GeForce RTX 5070 Ti, the 4K check produced zero error for the test
transform; median shader time was 0.0360 ms without a LUT and 0.0688 ms with a
33-point LUT (0.0328 ms added). This isolates shader overhead; it is not an
end-to-end playback benchmark or a guarantee for other hardware.
