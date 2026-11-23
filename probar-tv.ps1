param(
    [switch]$tv,
    [switch]$phone,
    [switch]$universal,
    [switch]$clean,
    [switch]$rebuild,
    [string]$ip = "192.168.0.51:37579"
)

$ErrorActionPreference = "Stop"

$ndkVer = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\ndk" | Select-Object -Last 1 -ExpandProperty FullName
$env:NDK_HOME = $ndkVer
$env:ANDROID_NDK_HOME = $ndkVer
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
$env:PATH = "$ndkVer\toolchains\llvm\prebuilt\windows-x86_64\bin;$env:JAVA_HOME\bin;$env:PATH"

# Variantes: -tv (TV 32 bits) | -phone (teléfono 64 bits) | -universal (64 + 32 bits, sirve en todo) | sin nada: emulador
if ($tv)            { $targets = @("armv7");            $apkSigned = "NextAnime-TV-1.0.apk" }
elseif ($phone)     { $targets = @("aarch64");          $apkSigned = "NextAnime-Phone-1.0.apk" }
elseif ($universal) { $targets = @("aarch64", "armv7"); $apkSigned = "NextAnime-Universal-1.0.apk" }
else                { $targets = @("i686");             $apkSigned = "NextAnime-Emu-1.0.apk" }
$target = $targets -join " + "
$pkg = "com.nextanime.tv"
$pkgViejo = "com.crunchyroll.casero.tv"

# Limpieza del nombre anterior: borra la carpeta Kotlin vieja (solo si los .kt nuevos ya están)
$java = "src-tauri\gen\android\app\src\main\java\com"
$kt = "$java\nextanime\tv"
if (!(Test-Path "$kt\MainActivity.kt") -or !(Test-Path "$kt\PlayerActivity.kt")) {
    Write-Host "Faltan MainActivity.kt / PlayerActivity.kt en $kt. Muévelos ahí primero." -ForegroundColor Red
    exit 1
}
if (Test-Path "$java\crunchyroll") {
    Remove-Item "$java\crunchyroll" -Recurse -Force
    Write-Host "Carpeta Kotlin vieja eliminada." -ForegroundColor Yellow
}
# -rebuild: borra la compilación anterior de Android (úsalo tras cambiar de ID o de archivos de gen)
if ($rebuild -and (Test-Path "src-tauri\gen\android\app\build")) {
    Remove-Item "src-tauri\gen\android\app\build" -Recurse -Force
    Write-Host "Compilación anterior eliminada." -ForegroundColor Yellow
}

Write-Host "Compilando para $target..." -ForegroundColor Cyan
npx tauri android build --apk --target @targets

$apkUnsigned = "src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release-unsigned.apk"
$keystore = "crunchy-tv-debug.keystore"
$buildTools = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\build-tools" | Select-Object -Last 1 -ExpandProperty FullName
$adb = "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe"

& "$buildTools\apksigner.bat" sign --ks $keystore --ks-pass pass:android --key-pass pass:android --out $apkSigned $apkUnsigned

if ($tv) {
    & $adb disconnect | Out-Null
    & $adb connect $ip
    if ($clean) {
        Write-Host "Limpiando cache y reinstalando desde cero en TV..." -ForegroundColor Yellow
        # Si la app no está instalada, adb responde "Failed": no es un error real
        $ErrorActionPreference = "Continue"
        & $adb -s $ip shell pm clear $pkg 2>&1 | Out-Null
        & $adb -s $ip uninstall $pkg 2>&1 | Out-Null
        & $adb -s $ip uninstall $pkgViejo 2>&1 | Out-Null
        $ErrorActionPreference = "Stop"
    }
    & $adb -s $ip install -r $apkSigned
} elseif ($phone -or $universal) {
    Write-Host "APK firmado: $(Resolve-Path $apkSigned)" -ForegroundColor Green
    Write-Host "Cópialo al teléfono y ábrelo para instalar (sin instalación automática)." -ForegroundColor Cyan
    return
} else {
    $emu = "emulator-5554"
    if ($clean) {
        Write-Host "Limpiando cache y reinstalando desde cero en Emulador..." -ForegroundColor Yellow
        $ErrorActionPreference = "Continue"
        & $adb -s $emu shell pm clear $pkg 2>&1 | Out-Null
        & $adb -s $emu uninstall $pkg 2>&1 | Out-Null
        & $adb -s $emu uninstall $pkgViejo 2>&1 | Out-Null
        $ErrorActionPreference = "Stop"
    }
    & $adb -s $emu logcat -c
    & $adb -s $emu install -r $apkSigned
    & $adb -s $emu shell monkey -p $pkg -c android.intent.category.LEANBACK_LAUNCHER 1
}

Write-Host "¡Listo! App instalada y ejecutándose." -ForegroundColor Green
