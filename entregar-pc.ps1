# Compila la app para PC (Windows) y deja el .exe en el Escritorio.
# Uso (desde la raiz del proyecto):  .\entregar-pc.ps1
#      con instalador (.exe setup):  .\entregar-pc.ps1 -instalador
param([switch]$instalador)

$ErrorActionPreference = "Continue"

if (-not (Test-Path ".\src-tauri\tauri.conf.json")) {
    Write-Host "Ejecuta este script desde la raiz del proyecto (donde esta src-tauri)." -ForegroundColor Red
    exit 1
}

$dest = Join-Path ([Environment]::GetFolderPath("Desktop")) "NextAnime-PC"
New-Item -ItemType Directory -Force -Path $dest | Out-Null

if ($instalador) {
    Write-Host "Compilando para PC con instalador..." -ForegroundColor Cyan
    npx tauri build
} else {
    Write-Host "Compilando para PC (solo .exe, sin instalador)..." -ForegroundColor Cyan
    npx tauri build --no-bundle
}
if ($LASTEXITCODE -ne 0) {
    Write-Host "FALLO la compilacion. Revisa el error de arriba." -ForegroundColor Red
    exit 1
}

# El exe se llama como el paquete de Cargo (tauri-app.exe)
$exe = Get-ChildItem ".\src-tauri\target\release\*.exe" -ErrorAction SilentlyContinue |
       Where-Object { $_.Name -notmatch "build|uninstall" } |
       Sort-Object LastWriteTime -Descending | Select-Object -First 1

if ($exe) {
    Copy-Item $exe.FullName (Join-Path $dest "NextAnime-1.0.exe") -Force
    Write-Host "OK    NextAnime-1.0.exe" -ForegroundColor Green
} else {
    Write-Host "FALLO no encontre el .exe en src-tauri\target\release" -ForegroundColor Red
}

if ($instalador) {
    Get-ChildItem ".\src-tauri\target\release\bundle" -Recurse -Include *.exe,*.msi -ErrorAction SilentlyContinue |
        ForEach-Object {
            Copy-Item $_.FullName $dest -Force
            Write-Host "OK    $($_.Name)" -ForegroundColor Green
        }
}

Start-Process explorer.exe $dest
