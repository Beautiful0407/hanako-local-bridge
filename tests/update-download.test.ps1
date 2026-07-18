$ErrorActionPreference = "Stop"

function Assert-UpdateDownload {
  param(
    [bool]$Condition,
    [string]$Message
  )
  if (-not $Condition) { throw $Message }
}

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$testRoot = Join-Path $env:TEMP "HanakoUpdateDownload-$([Guid]::NewGuid().ToString('N'))"
$sourcePath = Join-Path $testRoot "source.bin"
$destinationPath = Join-Path $testRoot "download.bin"
$serverScriptPath = Join-Path $testRoot "server.cjs"
$serverProcess = $null

try {
  New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
  $bytes = [byte[]]::new(262144)
  for ($index = 0; $index -lt $bytes.Length; $index++) {
    $bytes[$index] = $index % 251
  }
  [System.IO.File]::WriteAllBytes($sourcePath, $bytes)
  [System.IO.File]::WriteAllText(
    $serverScriptPath,
    @'
const fs = require("node:fs");
const http = require("node:http");
const port = Number(process.argv[2]);
const file = process.argv[3];
const payload = fs.readFileSync(file);
http.createServer((_request, response) => {
  response.writeHead(200, {
    "Content-Type": "application/octet-stream",
    "Content-Length": payload.length,
  });
  response.end(payload);
}).listen(port, "127.0.0.1");
'@,
    [System.Text.UTF8Encoding]::new($false)
  )

  $listener = [System.Net.Sockets.TcpListener]::new(
    [System.Net.IPAddress]::Loopback,
    0
  )
  $listener.Start()
  $port = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
  $listener.Stop()

  $node = (Get-Command node.exe -ErrorAction Stop).Source
  $serverProcess = Start-Process `
    -FilePath $node `
    -ArgumentList @(
      "`"$serverScriptPath`"",
      $port,
      "`"$sourcePath`""
    ) `
    -WindowStyle Hidden `
    -PassThru

  $deadline = (Get-Date).AddSeconds(10)
  do {
    Start-Sleep -Milliseconds 100
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
      $client.Connect("127.0.0.1", $port)
      $ready = $true
    } catch {
      $ready = $false
    } finally {
      $client.Dispose()
    }
  } while (-not $ready -and (Get-Date) -lt $deadline)
  Assert-UpdateDownload $ready "Local update download test server did not start."

  . (Join-Path $projectRoot "update-download.ps1")
  Save-HanakoUpdateResource `
    -Source "http://127.0.0.1:$port/package.bin" `
    -Destination $destinationPath `
    -TimeoutSeconds 30

  $sourceHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash
  $destinationHash = (Get-FileHash -LiteralPath $destinationPath -Algorithm SHA256).Hash
  Assert-UpdateDownload ($sourceHash -eq $destinationHash) "Downloaded update bytes do not match the source."
  Write-Host "update download tests passed"
} finally {
  if ($serverProcess -and -not $serverProcess.HasExited) {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
  }
  $resolvedTemp = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\"
  $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
  if ($resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
