param(
    [switch]$tv,
    [string]$ip = "192.168.0.51:44759"
)

$ErrorActionPreference = "Stop"

$ndkVer = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\ndk" | Select-Object -Last 1 -ExpandProperty FullName
$env:NDK_HOME = $ndkVer
$env:ANDROID_NDK_HOME = $ndkVer
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
$env:PATH = "$ndkVer\toolchains\llvm\prebuilt\windows-x86_64\bin;$env:JAVA_HOME\bin;$env:PATH"

$target = if ($tv) { "armv7" } else { "i686" }
$apkSigned = if ($tv) { "Crunchyroll-Casero-TV.apk" } else { "Crunchyroll-Casero-Emu.apk" }

Write-Host "Compilando para $target..." -ForegroundColor Cyan
npx tauri android build --apk --target $target

$apkUnsigned = "src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release-unsigned.apk"
$keystore = "crunchy-tv-debug.keystore"
$buildTools = Get-ChildItem "$env:LOCALAPPDATA\Android\Sdk\build-tools" | Select-Object -Last 1 -ExpandProperty FullName
$adb = "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe"

& "$buildTools\apksigner.bat" sign --ks $keystore --ks-pass pass:android --key-pass pass:android --out $apkSigned $apkUnsigned

if ($tv) {
    & $adb disconnect | Out-Null
    & $adb connect $ip
    & $adb -s $ip install -r $apkSigned
} else {
    & $adb -s emulator-5554 install -r $apkSigned
    & $adb -s emulator-5554 shell monkey -p com.crunchyroll.casero.tv -c android.intent.category.LEANBACK_LAUNCHER 1
}

Write-Host "¡Listo! App instalada y ejecutándose." -ForegroundColor Green
