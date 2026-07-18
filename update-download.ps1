function Save-HanakoUpdateResource {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination,
    [int]$TimeoutSeconds = 600
  )

  Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
  if ($Source -match "^https?://") {
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($curl) {
      $curlOutput = @(
        & $curl.Source `
          --fail `
          --location `
          --silent `
          --show-error `
          --connect-timeout 30 `
          --max-time $TimeoutSeconds `
          --retry 3 `
          --retry-delay 2 `
          --output $Destination `
          $Source 2>&1
      )
      if ($LASTEXITCODE -ne 0) {
        throw "curl.exe failed to download the update package: $($curlOutput -join ' ')"
      }
    } else {
      Invoke-WebRequest `
        -UseBasicParsing `
        -Uri $Source `
        -OutFile $Destination `
        -TimeoutSec $TimeoutSeconds
    }
  } else {
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
  }

  if (
    -not (Test-Path -LiteralPath $Destination -PathType Leaf) -or
    (Get-Item -LiteralPath $Destination).Length -le 0
  ) {
    throw "Update package download produced an empty file."
  }
}
