[CmdletBinding()]
param(
  [string]$BundleRoot = (Join-Path $PSScriptRoot "..\src-tauri\target\release\bundle"),
  [switch]$RequireSigned
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $BundleRoot -PathType Container)) {
  throw "Bundle directory does not exist: $BundleRoot. Run 'npm run bundle:windows' first."
}

$resolvedRoot = (Resolve-Path -LiteralPath $BundleRoot).Path
$msiArtifacts = @(Get-ChildItem -LiteralPath $resolvedRoot -Recurse -File -Filter "*.msi")
$nsisArtifacts = @(
  Get-ChildItem -LiteralPath $resolvedRoot -Recurse -File -Filter "*.exe" |
    Where-Object { $_.Name -like "*-setup.exe" }
)

if ($msiArtifacts.Count -eq 0) {
  throw "No MSI artifact found under $resolvedRoot."
}

if ($nsisArtifacts.Count -eq 0) {
  throw "No NSIS *-setup.exe artifact found under $resolvedRoot."
}

$artifacts = @($msiArtifacts + $nsisArtifacts) | Sort-Object FullName -Unique
$results = foreach ($artifact in $artifacts) {
  $signature = Get-AuthenticodeSignature -LiteralPath $artifact.FullName
  if ($RequireSigned -and $signature.Status -ne "Valid") {
    throw "Artifact is not validly signed: $($artifact.FullName) ($($signature.Status))."
  }

  $hash = Get-FileHash -LiteralPath $artifact.FullName -Algorithm SHA256
  [pscustomobject]@{
    Artifact = $artifact.FullName.Substring($resolvedRoot.Length).TrimStart("\")
    Bytes = $artifact.Length
    Signature = $signature.Status
    SHA256 = $hash.Hash.ToLowerInvariant()
  }
}

$results | Format-Table -AutoSize -Wrap

Write-Host "Verified $($msiArtifacts.Count) MSI and $($nsisArtifacts.Count) NSIS artifact(s)."
if (-not $RequireSigned) {
  Write-Host "Signing was reported but not required. Release builds must rerun with -RequireSigned after signing is configured."
}
