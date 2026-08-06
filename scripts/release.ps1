# Builds the Windows release binary, zips it, and writes install/scoop/deectx.json
# with the real SHA-256. Run from the repo root:
#   powershell -ExecutionPolicy Bypass -File scripts/release.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release
$bin = Join-Path $root "target\release\deectx.exe"
if (-not (Test-Path $bin)) { throw "release binary missing: $bin" }

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$zip = Join-Path $dist "deectx-x86_64-pc-windows-msvc.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $bin -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()

$manifest = @{
  version = "0.2.0"
  description = "Local-first PII-masking proxy for AI coding tools"
  homepage = "https://github.com/deectxone/deectx"
  license = "Apache-2.0"
  architecture = @{
    "64bit" = @{
      url = "https://github.com/deectxone/deectx/releases/download/v0.2.0/deectx-x86_64-pc-windows-msvc.zip"
      hash = $hash
    }
  }
  bin = "deectx.exe"
  checkver = @{ github = "https://github.com/deectxone/deectx" }
  autoupdate = @{
    architecture = @{
      "64bit" = @{ url = "https://github.com/deectxone/deectx/releases/download/v`$version/deectx-x86_64-pc-windows-msvc.zip" }
    }
  }
} | ConvertTo-Json -Depth 5

# Set-Content -Encoding UTF8 in Windows PowerShell 5.1 writes a UTF-8 BOM which
# breaks JSON parsing; write bytes explicitly as UTF-8 without BOM.
[System.IO.File]::WriteAllText((Join-Path $root "install\scoop\deectx.json"), $manifest, [System.Text.UTF8Encoding]::new($false))

Write-Host "Built $zip"
Write-Host "SHA-256: $hash"
Write-Host "Scoop manifest written to install/scoop/deectx.json"
Write-Host "Next: tag v0.2.0, attach $zip to the GitHub release, then:"
Write-Host "  gh release create v0.2.0 $zip --title 'v0.2.0' --notes '...'"
Write-Host "  cargo publish  (from a clean checkout)"
