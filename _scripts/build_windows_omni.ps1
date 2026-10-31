param([switch]$Release, [switch]$NoAiOpticalFlow)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskQtBin = Join-Path $taskRoot 'ext/6.7.3/msvc2019_64/bin'
$taskOpenCv = Join-Path $taskRoot 'ext/vcpkg/installed/x64-windows'
$taskFfmpeg = Join-Path $taskRoot 'ext/ffmpeg-9.0-windows-desktop-vs2026-gpl-lite'
$taskMdk = if (Test-Path -LiteralPath "$taskRoot/ext/mdk-nightly/mdk-sdk/lib/x64/mdk.lib") { "$taskRoot/ext/mdk-nightly/mdk-sdk" } else { "$taskRoot/ext/mdk-sdk" }
$taskLlvm = if ($env:LIBCLANG_PATH) { $env:LIBCLANG_PATH } else { 'C:/Program Files/LLVM/bin' }
foreach ($taskRequired in @("$taskQtBin/qmake.exe", "$taskOpenCv/lib/opencv_core4.lib", "$taskFfmpeg/lib/avcodec.lib", "$taskLlvm/clang.exe", "$taskMdk/lib/x64/mdk.lib")) {
    if (-not (Test-Path -LiteralPath $taskRequired)) { throw "Missing build dependency: $taskRequired" }
}
$env:QMAKE = "$taskQtBin/qmake.exe"
$env:FFMPEG_DIR = $taskFfmpeg
$env:FFMPEG_ARCH = 'x64'
$env:MDK_SDK = $taskMdk
$env:LIBCLANG_PATH = $taskLlvm
$env:CLANG_PATH = "$taskLlvm/clang.exe"
$env:OPENCV_LINK_LIBS = 'opencv_core4,opencv_calib3d4,opencv_features2d4,opencv_imgproc4,opencv_video4,opencv_flann4'
$env:OPENCV_LINK_PATHS = "$taskOpenCv/lib"
$env:OPENCV_INCLUDE_PATHS = "$taskOpenCv/include/opencv4"
$env:PATH = "$taskQtBin;$taskFfmpeg/bin;$taskFfmpeg/bin/x64;$taskOpenCv/bin;$taskLlvm;C:/Program Files/7-Zip;$env:PATH"
$env:RUSTFLAGS = "-L $taskRoot/ext/vcpkg/installed/x64-windows/lib"
# Keep the issued key out of source control while embedding it in local builds.
$taskLicenseFile = Join-Path $taskRoot 'ext/mdk-license-key.txt'
if (-not $env:GYROFLOW_OMNI_MDK_KEY -and (Test-Path -LiteralPath $taskLicenseFile)) {
    $env:GYROFLOW_OMNI_MDK_KEY = [IO.File]::ReadAllText($taskLicenseFile).Trim()
}
$taskArgs = @('build', '--locked')
if ($Release) { $taskArgs += '--release' }
if ($NoAiOpticalFlow) { $taskArgs += @('--no-default-features', '--features', 'opencv') }
Push-Location $taskRoot
try {
    & cargo @taskArgs
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed with exit code $LASTEXITCODE" }
} finally { Pop-Location }