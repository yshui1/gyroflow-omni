# Calibration LUTs

These nine non-ETTR `.cube` LUTs were copied unchanged from OmniCineControl's
`3dlut` directory. They are embedded in the standalone application, so selecting
a built-in LUT does not depend on a separate resource directory at runtime.

ETTR variants are intentionally excluded. The filenames identify the source
camera/profile; these transforms must match the gamma and gamut of the decoded
footage. A transform for one camera family is not automatically suitable for
another camera using a similarly named log curve.

See [calibration LUT usage](../../docs/calibration-luts.md).
