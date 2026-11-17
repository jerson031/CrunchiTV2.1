# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# Preservar información de líneas para depuración en Logcat
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile

# =====================================================================
# PUENTES JAVASCRIPT (@JavascriptInterface) PARA WEBVIEW DE TAURI
# =====================================================================
-keepattributes *Annotation*
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}
-keep class com.crunchyroll.casero.tv.** { *; }

# =====================================================================
# MEDIA3 EXOPLAYER & EXTRACTORS (.MKV, AV1, HEVC, SUBTÍTULOS)
# =====================================================================
-keep class androidx.media3.** { *; }
-dontwarn androidx.media3.**

# =====================================================================
# NEXTLIB (DECODIFICADORES FFMPEG: x265 10-BIT, AV1, AC3/EAC3, DTS)
# =====================================================================
-keep class io.github.anilbeesetti.nextlib.** { *; }
-dontwarn io.github.anilbeesetti.nextlib.**

# =====================================================================
# TAURI / WRY JNI BINDINGS (EVITA QUE R8 RECORTE MÉTODOS NATIVOS DE RUST)
# =====================================================================
-keep class app.tauri.** { *; }
-dontwarn app.tauri.**
