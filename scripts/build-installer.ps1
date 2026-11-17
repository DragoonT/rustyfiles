$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
$buildDir = Join-Path $root "target\installer"
$publishDir = Join-Path $buildDir "publish"
$releasesDir = Join-Path $buildDir "Releases"
$vpkVersion = "1.2.161"

Set-Location $root

$metadataJson = & rustup run stable cargo metadata --no-deps --format-version 1
if ($LASTEXITCODE -ne 0) { throw "Could not read the Cargo package version." }
$metadata = ($metadataJson -join "`n") | ConvertFrom-Json
$app = $metadata.packages | Where-Object { $_.name -eq "rf-app" } | Select-Object -First 1
if (-not $app) { throw "The rf-app package was not found in Cargo metadata." }
$version = $app.version

$tools = & dotnet tool list --global
if ($LASTEXITCODE -ne 0) { throw "Could not inspect installed .NET tools." }
$vpkRow = $tools | Select-String '^\s*vpk\s+'
if (-not $vpkRow) {
    & dotnet tool install --global vpk --version $vpkVersion
    if ($LASTEXITCODE -ne 0) { throw "Could not install Velopack CLI $vpkVersion." }
} elseif (($vpkRow.Line -split '\s+')[1] -ne $vpkVersion) {
    & dotnet tool update --global vpk --version $vpkVersion
    if ($LASTEXITCODE -ne 0) { throw "Could not update Velopack CLI to $vpkVersion." }
}

$vpk = Get-Command vpk -ErrorAction SilentlyContinue
if ($vpk) {
    $vpkPath = $vpk.Source
} else {
    $vpkPath = Join-Path $env:USERPROFILE ".dotnet\tools\vpk.exe"
}
if (-not (Test-Path $vpkPath)) { throw "Velopack CLI was not found at $vpkPath." }

Write-Host "Building rustyfiles $version..."
& rustup run stable cargo build --release -p rf-app -p rf-dialog -p rf-shellguard
if ($LASTEXITCODE -ne 0) { throw "The release build failed." }

$requiredFiles = @(
    (Join-Path $root "target\release\rustyfiles.exe"),
    (Join-Path $root "target\release\directlook.exe"),
    (Join-Path $root "target\release\rustyfiles-shellguard.exe"),
    (Join-Path $root "target\release\rustyfiles_dialog.dll"),
    (Join-Path $root "LICENSE")
)
foreach ($file in $requiredFiles) {
    if (-not (Test-Path $file)) { throw "Required installer file not found: $file" }
}

New-Item -ItemType Directory -Path $publishDir -Force | Out-Null
foreach ($file in $requiredFiles) {
    Copy-Item $file $publishDir -Force
}

New-Item -ItemType Directory -Path $buildDir -Force | Out-Null
if (Test-Path $releasesDir) {
    Remove-Item $releasesDir -Recurse -Force
}
Push-Location $buildDir
try {
    & $vpkPath pack -u rustyfiles -v $version -p $publishDir -e rustyfiles.exe -r win-x64 `
        --packTitle rustyfiles --packAuthors ("{0}zer G{1}zel" -f [char]0x00D6, [char]0x00FC) `
        -i (Join-Path $root "crates\rf-app\assets\rustyfiles.ico") `
        -s (Join-Path $root "crates\rf-app\assets\splash.gif") `
        --splashProgressColor "#ff6a1a"
    if ($LASTEXITCODE -ne 0) { throw "Velopack failed to create the installer." }
} finally {
    Pop-Location
}

if (-not (Test-Path $releasesDir)) { throw "Velopack did not create $releasesDir." }
$setup = Get-ChildItem $releasesDir -Filter "*Setup.exe" -Recurse | Select-Object -First 1
if (-not $setup) { throw "Velopack did not create a Setup.exe in $releasesDir." }

Write-Host ""
Write-Host "Installer created: $($setup.FullName)"
Write-Host "Uninstaller is registered by the installer in Windows Settings > Apps."
