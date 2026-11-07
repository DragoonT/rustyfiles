# Installs rustyfiles (and DirectLook, its Quick Look companion) for the current user (no admin rights needed):
#   - from a release zip: uses rustyfiles.exe and directlook.exe next to this script
#   - from the source tree: builds the release binaries first (unless -NoBuild)
#   - copies both to %LOCALAPPDATA%\Programs\rustyfiles
#   - creates a Start Menu shortcut (and a desktop shortcut with -Desktop)
#   - -Default: also makes it the default file manager (folders and Win+E open in rustyfiles;
#     same HKCU keys as Settings > Advanced, undone there or by -Uninstall)
# Uninstall: install.ps1 -Uninstall
param(
    [switch]$NoBuild,
    [switch]$Desktop,
    [switch]$Default,
    [switch]$Uninstall
)
$classes = "HKCU:\Software\Classes"
$winE = "$classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}"
$fmKeys = @("$classes\Folder\shell\open", "$classes\Folder\shell\explore", $winE)
# zip (CompressedFolder) / 7z, rar, tar… (ArchiveFolder) verbs: older versions pointed them here
$archiveKeys = @("$classes\CompressedFolder\shell\open", "$classes\ArchiveFolder\shell\open")
function Set-Verb($key, $command) {
    New-Item -Path $key -Force | Out-Null
    Set-ItemProperty -Path $key -Name "(default)" -Value $command
    Set-ItemProperty -Path $key -Name "DelegateExecute" -Value ""
}
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $env:LOCALAPPDATA "Programs\rustyfiles"
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\rustyfiles.lnk"
$desktopLnk = Join-Path ([Environment]::GetFolderPath("Desktop")) "rustyfiles.lnk"

if ($Uninstall) {
    Get-Process rustyfiles, directlook -ErrorAction SilentlyContinue | Stop-Process
    Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "DirectLook" -ErrorAction SilentlyContinue
    # default file manager: only our own overrides (folders must keep opening after uninstall)
    $open = (Get-ItemProperty "$classes\Folder\shell\open\command" -ErrorAction SilentlyContinue).'(default)'
    if ($open -like "*rustyfiles*") { $fmKeys | ForEach-Object { Remove-Item $_ -Recurse -Force -ErrorAction SilentlyContinue } }
    foreach ($k in $archiveKeys) {
        if ((Get-ItemProperty "$k\command" -ErrorAction SilentlyContinue).'(default)' -like "*rustyfiles*") {
            Remove-Item $k -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    # "opens archives" app registration (Default apps / Open with), made by the app itself
    Remove-Item "$classes\rustyfiles.Archive" -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($e in '.zip', '.7z', '.rar', '.tar', '.tgz', '.txz', '.tzst', '.tbz2', '.tbz') {
        Remove-ItemProperty "$classes\$e\OpenWithProgids" -Name "rustyfiles.Archive" -ErrorAction SilentlyContinue
    }
    Remove-Item "HKCU:\Software\rustyfiles" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-ItemProperty "HKCU:\Software\RegisteredApplications" -Name "rustyfiles" -ErrorAction SilentlyContinue
    # Open/Save dialogs: Windows' own come back (only our registrations go)
    foreach ($c in '{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}', '{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}') {
        if ((Get-ItemProperty "$classes\CLSID\$c\InprocServer32" -ErrorAction SilentlyContinue).'(default)' -like "*rustyfiles_dialog.dll") {
            Remove-Item "$classes\CLSID\$c" -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    Remove-Item $startMenu, $desktopLnk -ErrorAction SilentlyContinue
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "rustyfiles was removed. Settings stay in %APPDATA%\rustyfiles; delete that folder to remove them too."
    return
}

# a release zip has the programs next to this script; the source tree builds them
$zip = (Test-Path (Join-Path $PSScriptRoot "rustyfiles.exe")) -and (Test-Path (Join-Path $PSScriptRoot "directlook.exe"))
if ($zip) {
    $exe = Join-Path $PSScriptRoot "rustyfiles.exe"
    $dlExe = Join-Path $PSScriptRoot "directlook.exe"
    $dll = Join-Path $PSScriptRoot "rustyfiles_dialog.dll"
} else {
    if (-not $NoBuild) {
        Push-Location $root
        cargo build --release -p rf-app -p rf-dialog
        if ($LASTEXITCODE -ne 0) { Pop-Location; throw "The build failed." }
        Pop-Location
    }
    $exe = Join-Path $root "target\release\rustyfiles.exe"
    $dlExe = Join-Path $root "target\release\directlook.exe"
    $dll = Join-Path $root "target\release\rustyfiles_dialog.dll"
}
foreach ($e in @($exe, $dlExe, $dll)) { if (-not (Test-Path $e)) { throw "Not found: $e (build with: cargo build --release -p rf-app -p rf-dialog)" } }

New-Item -ItemType Directory $dest -Force | Out-Null
$running = @(Get-Process rustyfiles, directlook -ErrorAction SilentlyContinue)
$running | Stop-Process -Force -ErrorAction SilentlyContinue
# Stop-Process doesn't wait: the exe stays locked until the process is really gone
$running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
for ($try = 1; ; $try++) {
    try { Copy-Item $exe, $dlExe $dest -Force -ErrorAction Stop; break }
    catch {
        if ($try -ge 10) { throw }
        Start-Sleep -Milliseconds 500
    }
}
# The dialog DLL may be loaded in running apps (when Open/Save dialogs are on): a loaded DLL
# can't be overwritten but can be renamed. Apps keep the old one; new dialogs load the new.
Get-ChildItem $dest -Filter "rustyfiles_dialog.old-*.dll" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
$dllDest = Join-Path $dest "rustyfiles_dialog.dll"
try { Copy-Item $dll $dllDest -Force -ErrorAction Stop }
catch {
    Rename-Item $dllDest ("rustyfiles_dialog.old-{0}.dll" -f [DateTime]::Now.Ticks) -ErrorAction Stop
    Copy-Item $dll $dllDest -Force -ErrorAction Stop
}

# Windows caches an exe's display name; drop ours so a new FileDescription shows up
$mui = "HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache"
foreach ($n in 'rustyfiles.exe', 'directlook.exe') {
    foreach ($v in 'FriendlyAppName', 'ApplicationCompany') {
        Remove-ItemProperty $mui -Name "$(Join-Path $dest $n).$v" -ErrorAction SilentlyContinue
    }
}

$shell = New-Object -ComObject WScript.Shell
foreach ($lnkPath in @($startMenu) + $(if ($Desktop) { @($desktopLnk) } else { @() })) {
    $lnk = $shell.CreateShortcut($lnkPath)
    $lnk.TargetPath = Join-Path $dest "rustyfiles.exe"
    $lnk.WorkingDirectory = $dest
    $lnk.Description = "rustyfiles - a fast file manager"
    $lnk.Save()
}
if ($Default) {
    $app = Join-Path $dest "rustyfiles.exe"
    # back up whatever per-user folder verbs exist before overwriting them (once)
    $backup = Join-Path $dest "klasor-acma-yedek.reg"
    $open = (Get-ItemProperty "$classes\Folder\shell\open\command" -ErrorAction SilentlyContinue).'(default)'
    if ((Test-Path "$classes\Folder\shell") -and $open -notlike "*rustyfiles*" -and -not (Test-Path $backup)) {
        reg.exe export "HKCU\Software\Classes\Folder\shell" $backup /y | Out-Null
        Write-Host "Your previous folder settings were backed up to $backup"
    }
    Set-Verb "$classes\Folder\shell\open\command" "`"$app`" `"%1`""
    Set-Verb "$classes\Folder\shell\explore\command" "`"$app`" `"%1`""
    Set-Verb "$winE\shell\opennewwindow\command" "`"$app`""
    # Windows' own archive handlers stay Explorer's (pointing them here merged "File Explorer"
    # and rustyfiles into one "Open with" entry); archives: Settings → Default apps → rustyfiles
    foreach ($k in $archiveKeys) {
        if ((Get-ItemProperty "$k\command" -ErrorAction SilentlyContinue).'(default)' -like "*rustyfiles*") {
            Remove-Item $k -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    Write-Host "Folders and Win+E now open in rustyfiles. To undo: Settings > Advanced, or install.ps1 -Uninstall."
}
# DirectLook was stopped for the copy: start it again (Space on the desktop and in Explorer
# needs it running; otherwise it only comes back at the next sign-in or rustyfiles launch)
$autostart = (Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -ErrorAction SilentlyContinue).DirectLook
if ($autostart -or ($running | Where-Object { $_.ProcessName -eq "directlook" })) {
    Start-Process -FilePath (Join-Path $dest "directlook.exe") -ArgumentList "--background"
    Write-Host "DirectLook was restarted."
}
Write-Host "Installed to $dest (rustyfiles.exe and directlook.exe)."
Write-Host "Start it from the Start menu: rustyfiles. Press Space on a file for DirectLook."
