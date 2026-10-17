# Installs the setup silently into a temp folder, checks it, then uninstalls it again.
param([Parameter(Mandatory = $true)][string]$Setup)
$ErrorActionPreference = 'Stop'

$dir = Join-Path ([IO.Path]::GetTempPath()) 'wield-install-test'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8E5B9C2A-6F4D-4A8B-9E2C-57D1A3B0C4F6}_is1'

$p = Start-Process $Setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/NOLAUNCH', "/DIR=$dir" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "Setup exited with $($p.ExitCode)" }
if (-not (Test-Path "$dir\wield.exe")) { throw 'wield.exe was not installed' }
if (-not (Test-Path $key)) { throw 'Uninstall entry is missing' }
$loc = (Get-ItemProperty $key).InstallLocation.TrimEnd('\')
if ($loc -ne $dir.TrimEnd('\')) { throw "InstallLocation is '$loc', expected '$dir'" }
$version = (Get-Item "$dir\wield.exe").VersionInfo.FileVersion
Write-Host "Installed Wield $version to $dir"

$p = Start-Process "$dir\unins000.exe" -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
Start-Sleep -Seconds 3
if (Test-Path "$dir\wield.exe") { throw 'Uninstall left wield.exe behind' }
if (Test-Path $key) { throw 'Uninstall left its registry entry behind' }
Write-Host 'Installer smoke test passed.'
