# Compila y firma los APK para llevar en pendrive. NO usa adb ni espera dispositivos.
# Deja los archivos en  Escritorio\NextAnime-APKs
# Uso (desde la raiz del proyecto):  .\entregar.ps1            -> universal + telefono
#                                    .\entregar.ps1 -rebuild   -> igual, partiendo de cero
param(
    [switch]$rebuild
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$version = "1.0"
$destino = Join-Path ([Environment]::GetFolderPath("Desktop")) "NextAnime-APKs"
New-Item -ItemType Directory -Force $destino | Out-Null

# --- Entorno Android (igual que probar-tv.ps1) ---
$ndkVer = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\ndk" | Select-Object -Last 1 -ExpandProperty FullName
$env:NDK_HOME = $ndkVer
$env:ANDROID_NDK_HOME = $ndkVer
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
$env:PATH = "$ndkVer\toolchains\llvm\prebuilt\windows-x86_64\bin;$env:JAVA_HOME\bin;$env:PATH"
$buildTools = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\build-tools" | Select-Object -Last 1 -ExpandProperty FullName
$keystore = "crunchy-tv-debug.keystore"
$apkUnsigned = "src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release-unsigned.apk"

# --- Comprobaciones ---
$java = "src-tauri\gen\android\app\src\main\java\com"
$kt = "$java\nextanime\tv"
if (!(Test-Path "$kt\MainActivity.kt") -or !(Test-Path "$kt\PlayerActivity.kt")) {
    Write-Host "Faltan MainActivity.kt / PlayerActivity.kt en $kt" -ForegroundColor Red
    exit 1
}
if (Test-Path "$java\crunchyroll") { Remove-Item "$java\crunchyroll" -Recurse -Force }
if ($rebuild -and (Test-Path "src-tauri\gen\android\app\build")) {
    Remove-Item "src-tauri\gen\android\app\build" -Recurse -Force
    Write-Host "Compilacion anterior eliminada." -ForegroundColor Yellow
}

# nombre -> targets de Rust
$variantes = @(
    @{ nombre = "Universal"; targets = @("aarch64", "armv7") },
    @{ nombre = "Phone";     targets = @("aarch64") }
)

$ok = @()
$fallo = @()

foreach ($v in $variantes) {
    $apk = "NextAnime-$($v.nombre)-$version.apk"
    $targets = $v.targets
    Write-Host "`n=== Compilando $($v.nombre) ($($targets -join ' + ')) ===" -ForegroundColor Cyan
    try {
        if (Test-Path $apkUnsigned) { Remove-Item $apkUnsigned -Force }
        npx tauri android build --apk --target @targets
        if ($LASTEXITCODE -ne 0) { throw "La compilacion termino con error ($LASTEXITCODE)" }
        if (!(Test-Path $apkUnsigned)) { throw "No se genero el APK sin firmar" }
        & "$buildTools\apksigner.bat" sign --ks $keystore --ks-pass pass:android --key-pass pass:android --out $apk $apkUnsigned
        if ($LASTEXITCODE -ne 0) { throw "Fallo la firma ($LASTEXITCODE)" }
        Copy-Item $apk $destino -Force
        $ok += $apk
    } catch {
        Write-Host "Fallo $($v.nombre): $($_.Exception.Message)" -ForegroundColor Red
        $fallo += $v.nombre
    }
}

Write-Host "`n=== Resumen ===" -ForegroundColor Cyan
foreach ($a in $ok) { Write-Host "OK    $a" -ForegroundColor Green }
foreach ($f in $fallo) { Write-Host "FALLO $f" -ForegroundColor Red }
if ($ok.Count -gt 0) {
    Write-Host "`nCarpeta: $destino" -ForegroundColor Green
    Invoke-Item $destino
}