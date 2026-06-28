$ErrorActionPreference = "Stop"

$ProjectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$VendorRoot = Join-Path $ProjectRoot "vendor\ffmpeg"
$BinDir = Join-Path $VendorRoot "bin"
$TempDir = Join-Path $ProjectRoot ".tmp\ffmpeg-download"
$Archive = Join-Path $TempDir "ffmpeg-release-essentials.zip"
$ShaFile = Join-Path $TempDir "ffmpeg-release-essentials.zip.sha256"
$DownloadUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
$ShaUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip.sha256"

New-Item -ItemType Directory -Force -Path $TempDir, $BinDir | Out-Null

Write-Host "Descargando FFmpeg portable..."
Invoke-WebRequest -Uri $DownloadUrl -OutFile $Archive
Invoke-WebRequest -Uri $ShaUrl -OutFile $ShaFile

$ExpectedHash = ((Get-Content $ShaFile -Raw).Trim() -split "\s+")[0].ToUpperInvariant()
$ActualHash = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToUpperInvariant()

if ($ExpectedHash -ne $ActualHash) {
    throw "La verificacion SHA256 fallo. Esperado $ExpectedHash, recibido $ActualHash."
}

$ExtractDir = Join-Path $TempDir "extract"
Remove-Item -LiteralPath $ExtractDir -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -LiteralPath $Archive -DestinationPath $ExtractDir -Force

$Ffmpeg = Get-ChildItem -LiteralPath $ExtractDir -Recurse -Filter "ffmpeg.exe" | Select-Object -First 1
$Ffprobe = Get-ChildItem -LiteralPath $ExtractDir -Recurse -Filter "ffprobe.exe" | Select-Object -First 1

if (-not $Ffmpeg -or -not $Ffprobe) {
    throw "No se encontraron ffmpeg.exe y ffprobe.exe dentro del zip descargado."
}

Copy-Item -LiteralPath $Ffmpeg.FullName -Destination (Join-Path $BinDir "ffmpeg.exe") -Force
Copy-Item -LiteralPath $Ffprobe.FullName -Destination (Join-Path $BinDir "ffprobe.exe") -Force

Remove-Item -LiteralPath $TempDir -Recurse -Force

Write-Host "Listo:"
Write-Host "  $(Join-Path $BinDir "ffmpeg.exe")"
Write-Host "  $(Join-Path $BinDir "ffprobe.exe")"
