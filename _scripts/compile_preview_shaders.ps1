param([string]$Qsb = "$PSScriptRoot/../ext/6.7.3/msvc2019_64/bin/qsb.exe")
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
New-Item -ItemType Directory -Path "$taskRoot/target" -Force | Out-Null
$taskOutput = Join-Path $taskRoot 'src/qt_gpu/compiled'
$taskTemplate = [IO.File]::ReadAllText("$taskRoot/src/qt_gpu/undistort.frag")
$taskModels = @('opencv_fisheye', 'opencv_standard', 'poly3', 'poly5', 'ptlens', 'insta360', 'sony', 'generic_polynomial', 'gopro')
$taskDigital = @('', 'gopro_superview', 'gopro6_superview', 'gopro_hyperview', 'gopro_warp', 'digital_stretch')
$taskTemp = Join-Path $taskRoot 'target/preview-shader.frag'
foreach ($taskModel in $taskModels) {
    foreach ($taskLens in $taskDigital) {
        if ($taskLens -in @('gopro_superview', 'gopro6_superview', 'gopro_hyperview') -and $taskModel -ne 'opencv_fisheye') { continue }
        if ($taskLens -eq 'gopro_warp' -and $taskModel -ne 'gopro') { continue }
        if ($taskModel -eq 'gopro' -and $taskLens -ne '' -and $taskLens -ne 'gopro_warp') { continue }
        $taskFunctions = if ($taskLens) { [IO.File]::ReadAllText("$taskRoot/src/core/stabilization/distortion_models/$taskLens.glsl") } else { 'vec2 digital_undistort_point(vec2 uv) { return uv; } vec2 digital_distort_point(vec2 uv) { return uv; }' }
        if ($taskModel -notin @('sony', 'generic_polynomial')) { $taskFunctions += ' vec2 process_coord(vec2 uv, float idx) { return uv; } ' }
        $taskFunctions += [IO.File]::ReadAllText("$taskRoot/src/core/stabilization/distortion_models/$taskModel.glsl")
        $taskShader = $taskTemplate.Replace('LENS_MODEL_FUNCTIONS;', $taskFunctions)
        if ($taskModel -in @('sony', 'generic_polynomial')) { $taskShader += "`nfloat get_mesh_data(int idx) { return texture(texMeshData, vec2(0, idx / 2047.0)).r; }" }
        [IO.File]::WriteAllText($taskTemp, $taskShader)
        $taskName = 'undistort_' + $taskModel + $(if ($taskLens) { '_' + $taskLens })
        foreach ($taskLut in @($false, $true)) {
            $taskArgs = @('--glsl', '120,300 es,310 es,320 es,310,320,330,400,410,420', '--hlsl', '50', '--msl', '12')
            $taskSuffix = if ($taskLut) { '_lut' } else { '' }
            if ($taskLut) { $taskArgs += @('-D', 'CALIBRATION_LUT') }
            & $Qsb @taskArgs -o "$taskOutput/$taskName$taskSuffix.frag.qsb" $taskTemp
            if ($LASTEXITCODE -ne 0) { throw "Shader compilation failed: $taskName$taskSuffix" }
        }
    }
}
