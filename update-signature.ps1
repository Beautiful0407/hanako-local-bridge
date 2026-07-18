function Get-HanakoUpdateSignaturePayload {
  param([Parameter(Mandatory = $true)]$ManifestData)

  @(
    "schemaVersion=$([string]$ManifestData.schemaVersion)"
    "channel=$([string]$ManifestData.channel)"
    "version=$([string]$ManifestData.version)"
    "packageUrl=$([string]$ManifestData.packageUrl)"
    "sha256=$(([string]$ManifestData.sha256).Trim().ToLowerInvariant())"
    "size=$([long]$ManifestData.size)"
  ) -join "`n"
}

function New-HanakoUpdateManifestSignature {
  param(
    [Parameter(Mandatory = $true)]$ManifestData,
    [Parameter(Mandatory = $true)][string]$PrivateKeyPath
  )

  if (-not (Test-Path -LiteralPath $PrivateKeyPath -PathType Leaf)) {
    throw "Update signing private key does not exist: $PrivateKeyPath"
  }
  $payload = Get-HanakoUpdateSignaturePayload -ManifestData $ManifestData
  $rsa = [System.Security.Cryptography.RSACryptoServiceProvider]::new()
  $sha256 = [System.Security.Cryptography.SHA256]::Create()
  try {
    $rsa.FromXmlString([System.IO.File]::ReadAllText($PrivateKeyPath))
    $signature = $rsa.SignData(
      [System.Text.Encoding]::UTF8.GetBytes($payload),
      $sha256
    )
    return [Convert]::ToBase64String($signature)
  } finally {
    $sha256.Dispose()
    $rsa.Dispose()
  }
}

function Assert-HanakoUpdateManifestSignature {
  param(
    [Parameter(Mandatory = $true)]$ManifestData,
    [Parameter(Mandatory = $true)][string]$PublicKeyPath,
    [switch]$Required
  )

  $signatureText = ([string]$ManifestData.signature).Trim()
  if ([string]::IsNullOrWhiteSpace($signatureText)) {
    if ($Required) { throw "Remote update manifest is not signed." }
    return $false
  }
  if ([string]$ManifestData.signatureAlgorithm -ne "RSA-SHA256") {
    throw "Unsupported update signature algorithm."
  }
  if (-not (Test-Path -LiteralPath $PublicKeyPath -PathType Leaf)) {
    throw "Update signing public key does not exist: $PublicKeyPath"
  }

  $payload = Get-HanakoUpdateSignaturePayload -ManifestData $ManifestData
  $rsa = [System.Security.Cryptography.RSACryptoServiceProvider]::new()
  $sha256 = [System.Security.Cryptography.SHA256]::Create()
  try {
    $rsa.FromXmlString([System.IO.File]::ReadAllText($PublicKeyPath))
    $valid = $rsa.VerifyData(
      [System.Text.Encoding]::UTF8.GetBytes($payload),
      $sha256,
      [Convert]::FromBase64String($signatureText)
    )
    if (-not $valid) { throw "Update manifest signature verification failed." }
    return $true
  } finally {
    $sha256.Dispose()
    $rsa.Dispose()
  }
}
