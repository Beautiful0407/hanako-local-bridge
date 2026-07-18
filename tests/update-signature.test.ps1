$ErrorActionPreference = "Stop"

function Assert-UpdateSignature {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$tempRoot = Join-Path $env:TEMP "HanakoUpdateSignatureTest-$([Guid]::NewGuid().ToString('N'))"
$privateKeyPath = Join-Path $tempRoot "private-key.xml"
$publicKeyPath = Join-Path $tempRoot "public-key.xml"

try {
  New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
  $rsa = [System.Security.Cryptography.RSACryptoServiceProvider]::new(2048)
  try {
    [System.IO.File]::WriteAllText($privateKeyPath, $rsa.ToXmlString($true))
    [System.IO.File]::WriteAllText($publicKeyPath, $rsa.ToXmlString($false))
  } finally {
    $rsa.Dispose()
  }

  . (Join-Path $projectRoot "update-signature.ps1")
  $manifest = [pscustomobject]@{
    schemaVersion = 1
    channel = "stable"
    version = "9.9.9"
    packageUrl = "https://example.invalid/HanakoLocalBridge-9.9.9-win-x64.zip"
    sha256 = ("ab" * 32)
    size = 123456
    signatureAlgorithm = "RSA-SHA256"
    signature = ""
  }
  $manifest.signature = New-HanakoUpdateManifestSignature `
    -ManifestData $manifest `
    -PrivateKeyPath $privateKeyPath
  Assert-HanakoUpdateManifestSignature `
    -ManifestData $manifest `
    -PublicKeyPath $publicKeyPath `
    -Required

  $tampered = $manifest | ConvertTo-Json -Depth 8 | ConvertFrom-Json
  $tampered.size = 123457
  $tamperRejected = $false
  try {
    Assert-HanakoUpdateManifestSignature `
      -ManifestData $tampered `
      -PublicKeyPath $publicKeyPath `
      -Required
  } catch {
    $tamperRejected = $true
  }
  Assert-UpdateSignature $tamperRejected "A tampered manifest passed signature verification."

  $unsigned = $manifest | ConvertTo-Json -Depth 8 | ConvertFrom-Json
  $unsigned.signature = ""
  $unsignedRejected = $false
  try {
    Assert-HanakoUpdateManifestSignature `
      -ManifestData $unsigned `
      -PublicKeyPath $publicKeyPath `
      -Required
  } catch {
    $unsignedRejected = $true
  }
  Assert-UpdateSignature $unsignedRejected "A required unsigned manifest was accepted."

  Write-Host "update signature tests passed"
} finally {
  if (Test-Path -LiteralPath $tempRoot) {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
