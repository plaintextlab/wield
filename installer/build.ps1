# Builds dist\wield-v<Version>-setup.exe from target\release\wield.exe with Inno Setup.
# Installs Inno Setup with Chocolatey when it isn't present (GitHub Windows runners have choco).
param([Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = 'Stop'

$iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    foreach ($p in @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe")) {
        if (Test-Path $p) { $iscc = $p; break }
    }
}
if (-not $iscc) {
    Write-Host 'Inno Setup not found; installing with Chocolatey...'
    choco install innosetup -y --no-progress | Out-Host
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
& $iscc "/DAppVersion=$Version" "$PSScriptRoot\wield.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
