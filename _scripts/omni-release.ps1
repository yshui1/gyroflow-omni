<#
.SYNOPSIS
Builds the Windows Gyroflow (Omni) app and uploads it to a draft GitHub release.

.DESCRIPTION
Omni has no CI, so releases are built locally on each OS:
  1. Windows: .\_scripts\omni-release.ps1            builds, creates the draft release v<version>, uploads Gyroflow-windows64.zip
  2. macOS:   ./_scripts/omni-release.sh             builds and uploads Gyroflow-mac-universal.dmg to the same draft
  3. Either:  .\_scripts\omni-release.ps1 -Publish   checks both files are there, then publishes the release

The app's update check only sees published releases, so nobody is told about a release until it's published.
The version comes from Cargo.toml. Bump it first (see the version list in the `publish` recipe in Justfile).

.PARAMETER NotesFile
Markdown file with the release notes, which the app shows in its "newer version available" popup.
Without it, GitHub generates notes from the commits.

.PARAMETER Publish
Publish the existing draft instead of building. Fails if any expected file is missing.

.PARAMETER SkipBuild
Upload the zip already in _deployment\_binaries without rebuilding.

.PARAMETER DryRun
Build and check the files, but don't change anything on GitHub.
#>
param(
    [string]$NotesFile,
    [switch]$Publish,
    [switch]$SkipBuild,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'yshui1/gyroflow-omni'
$Root = Split-Path $PSScriptRoot -Parent
$Binaries = Join-Path $Root '_deployment/_binaries'
$WindowsAssets = @('Gyroflow-windows64.zip')
$MacAssets     = @('Gyroflow-mac-universal.dmg')

function Invoke-Checked([string]$What, [scriptblock]$Block) {
    & $Block
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}
function Get-Release([string]$Tag) {
    # Windows PowerShell turns redirected native stderr into errors, which 'Stop' would throw on.
    $ErrorActionPreference = 'Continue'
    $json = gh release view $Tag -R $Repo --json isDraft,assets 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    $json | ConvertFrom-Json
}

foreach ($tool in 'gh', 'git') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool is not installed." }
}

$m = Select-String -LiteralPath (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $m) { throw 'No version in Cargo.toml' }
$Version = $m.Matches[0].Groups[1].Value
$Tag = "v$Version"
Write-Host "Gyroflow (Omni) $Tag" -ForegroundColor Cyan

if ($Publish) {
    $release = Get-Release $Tag
    if (-not $release) { throw "No release $Tag on $Repo. Build and upload first." }
    if (-not $release.isDraft) { Write-Host "$Tag is already published."; exit 0 }
    $have = @($release.assets | ForEach-Object { $_.name })
    $missing = @($WindowsAssets + $MacAssets | Where-Object { $have -notcontains $_ })
    if ($missing) { throw "Not publishing, the draft is missing: $($missing -join ', ')" }
    $answer = Read-Host "Publish $Tag to everyone (the app will show it as an update)? [y/N]"
    if ($answer -notmatch '^(y|yes)$') { Write-Host 'Cancelled.'; exit 1 }
    Invoke-Checked 'gh release edit' { gh release edit $Tag -R $Repo --draft=false --latest }
    Write-Host "Published https://github.com/$Repo/releases/tag/$Tag" -ForegroundColor Green
    exit 0
}

# The release tag is created at this commit, so it has to be on GitHub.
Push-Location $Root
try {
    $sha = (git rev-parse HEAD).Trim()
    if (git status --porcelain --untracked-files=no) { throw 'There are uncommitted changes. Commit them first so the release matches a commit.' }
    git fetch --quiet origin
    if (-not (git branch -r --contains $sha)) { throw "Commit $($sha.Substring(0, 8)) isn't pushed to origin. Push it first." }
} finally { Pop-Location }

if (-not $SkipBuild) {
    if (-not (Get-Command just -ErrorAction SilentlyContinue)) { throw 'just is not installed (winget install Casey.Just).' }
    if (-not $env:LIBCLANG_PATH -and (Test-Path 'C:\Program Files\LLVM\bin\libclang.dll')) { $env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin' }
    if (-not (Get-Command 7z -ErrorAction SilentlyContinue)) {
        if (Test-Path 'C:\Program Files\7-Zip\7z.exe') { $env:PATH = "C:\Program Files\7-Zip;$env:PATH" } else { throw '7-Zip is not installed (winget install 7zip.7zip).' }
    }
    $env:OMNI_RELEASE = '1'   # the app reports "1.0.0" instead of "1.0.0 (dev...)" and uses the release update path
    Push-Location $Root
    try {
        Invoke-Checked 'app build' { just deploy }
    } finally { Pop-Location; Remove-Item Env:OMNI_RELEASE }
}

$files = foreach ($name in $WindowsAssets) {
    $path = Join-Path $Binaries $name
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing $path" }
    $path
}
$exe = Join-Path $Binaries 'win64/gyroflow.exe'
if (Test-Path -LiteralPath $exe) {
    $reported = (& $exe --version | Out-String).Trim()
    if ($reported -and $reported -ne "Gyroflow (Omni) v$Version") { throw "gyroflow.exe reports '$reported', expected 'Gyroflow (Omni) v$Version'. Rebuild without -SkipBuild." }
    if (-not $reported) { Write-Warning "gyroflow.exe printed no version; check Help > About shows $Version." }
}
foreach ($f in $files) {
    $hash = (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash
    Write-Host ("  {0}  {1:N1} MB  SHA256 {2}" -f (Split-Path $f -Leaf), ((Get-Item $f).Length / 1MB), $hash)
}

if ($DryRun) { Write-Host 'Dry run: nothing uploaded.' -ForegroundColor Yellow; exit 0 }

$release = Get-Release $Tag
if ($release -and -not $release.isDraft) { throw "$Tag is already published. Bump the version for a new release." }
if (-not $release) {
    $notes = if ($NotesFile) { @('--notes-file', (Resolve-Path $NotesFile).Path) } else { @('--generate-notes') }
    Invoke-Checked 'gh release create' { gh release create $Tag -R $Repo --draft --target $sha --title "Gyroflow (Omni) $Tag" @notes }
} elseif ($NotesFile) {
    Invoke-Checked 'gh release edit' { gh release edit $Tag -R $Repo --notes-file (Resolve-Path $NotesFile).Path }
}
Invoke-Checked 'gh release upload' { gh release upload $Tag -R $Repo --clobber @files }

Write-Host "Uploaded to draft $Tag. Next: run ./_scripts/omni-release.sh on the Mac, then .\_scripts\omni-release.ps1 -Publish" -ForegroundColor Green
