package com.nextanime.tv

import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {

    private var webViewRef: WebView? = null
    private val mainHandler = Handler(Looper.getMainLooper())
    private var webViewAttached = false
    private var ignoreBackUntilMs: Long = 0L
    private var pendingOpenSourcesFromIntent = false
    @Volatile private var pendingOpenSubsMs: Long = -1L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        hideSystemBars()

        if (intent?.getBooleanExtra("OPEN_SOURCES", false) == true) {
            intent.removeExtra("OPEN_SOURCES")
            pendingOpenSourcesFromIntent = true
        }

        mainHandler.post {
            findViewById<ViewGroup>(android.R.id.content)?.let { rootContent ->
                attachWebViewBridge(rootContent, attempt = 0)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (intent.getBooleanExtra("OPEN_SOURCES", false)) {
            intent.removeExtra("OPEN_SOURCES")
            pendingOpenSourcesFromIntent = true
            if (webViewAttached) {
                triggerOpenSourcesModal()
            }
        }
    }

    private fun hideSystemBars() {
        val insetsController = WindowCompat.getInsetsController(window, window.decorView)
        insetsController.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        insetsController.hide(WindowInsetsCompat.Type.systemBars())
    }

    private fun attachWebViewBridge(root: ViewGroup, attempt: Int) {
        if (webViewAttached || attempt > 40) return

        mainHandler.postDelayed({
            if (webViewAttached) return@postDelayed
            val wv = findWebView(root)
            if (wv != null) {
                webViewRef = wv
                wv.addJavascriptInterface(ExoBridge(), "AndroidExo")
                wv.isFocusable = true
                wv.isFocusableInTouchMode = true
                wv.requestFocus()
               webViewAttached = true
               val flagFile = java.io.File(applicationContext.filesDir, "open_sources.flag")
               if (flagFile.exists() || pendingOpenSourcesFromIntent) {
               flagFile.delete()
               triggerOpenSourcesModal()
            }
               checkSubsFlagAndTrigger()
            } else {
                attachWebViewBridge(root, attempt + 1)
            }
        }, 150L)
    }

    private fun triggerOpenSourcesModal() {
        pendingOpenSourcesFromIntent = false
        mainHandler.postDelayed({
            webViewRef?.evaluateJavascript(
                """
                (function() {
                    let tries = 0;
                    function attemptOpen() {
                        if (typeof window.openSourcesForCurrentEpisode === 'function') {
                            window.openSourcesForCurrentEpisode();
                        } else if (tries < 60) {
                            tries++;
                            setTimeout(attemptOpen, 150);
                        }
                    }
                    attemptOpen();
                })();
                """.trimIndent(),
                null
            )
        }, 200L)
    }

    /** Lee open_subs.flag (posición en ms) escrito por PlayerActivity y abre el selector de subtítulos. */
    private fun takeSubsFlag(): Long {
        val f = java.io.File(applicationContext.filesDir, "open_subs.flag")
        if (!f.exists()) return -1L
        val ms = try { f.readText().trim().toLongOrNull() ?: 0L } catch (_: Exception) { 0L }
        f.delete()
        return if (ms < 0L) 0L else ms
    }

    private fun takeEpFlag(): Int {
        val f = java.io.File(applicationContext.filesDir, "play_ep.flag")
        if (!f.exists()) return 0
        val ep = try { f.readText().trim().toIntOrNull() ?: 0 } catch (_: Exception) { 0 }
        f.delete()
        return ep
    }

    private fun checkSubsFlagAndTrigger() {
        val ms = takeSubsFlag()
        if (ms >= 0L) pendingOpenSubsMs = ms
        if (pendingOpenSubsMs >= 0L && webViewAttached) {
            val v = pendingOpenSubsMs
            pendingOpenSubsMs = -1L
            triggerOpenSubsModal(v)
        }
    }

    private fun triggerOpenSubsModal(startMs: Long) {
        mainHandler.postDelayed({
            webViewRef?.evaluateJavascript(
                """
                (function() {
                    let tries = 0;
                    function attemptOpen() {
                        if (typeof window.openSubtitlePickerForCurrentEpisode === 'function') {
                            window.openSubtitlePickerForCurrentEpisode($startMs);
                        } else if (tries < 60) {
                            tries++;
                            setTimeout(attemptOpen, 150);
                        }
                    }
                    attemptOpen();
                })();
                """.trimIndent(),
                null
            )
        }, 200L)
    }

    private fun findWebView(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) {
            for (i in 0 until view.childCount) {
                val found = findWebView(view.getChildAt(i))
                if (found != null) return found
            }
        }
        return null
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_BACK ||
            event.keyCode == KeyEvent.KEYCODE_DPAD_CENTER ||
            event.keyCode == KeyEvent.KEYCODE_ENTER
        ) {
            if (SystemClock.uptimeMillis() < ignoreBackUntilMs) {
                return true
            }
            if (event.keyCode == KeyEvent.KEYCODE_BACK) {
                if (event.action == KeyEvent.ACTION_DOWN) {
                    webViewRef?.evaluateJavascript(
                        "window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape' }));",
                        null
                    )
                }
                return true
            }
        }
        return super.dispatchKeyEvent(event)
    }

        override fun onPause() {
        super.onPause()
        webViewRef?.onPause()
        webViewRef?.pauseTimers()
    }

    override fun onResume() {
        super.onResume()
        webViewRef?.onResume()
        webViewRef?.resumeTimers()
        hideSystemBars()
        ignoreBackUntilMs = SystemClock.uptimeMillis() + 600L

        if (intent?.getBooleanExtra("OPEN_SOURCES", false) == true) {
            intent.removeExtra("OPEN_SOURCES")
            pendingOpenSourcesFromIntent = true
        }

        val flagFile = java.io.File(applicationContext.filesDir, "open_sources.flag")
        android.util.Log.d("CR_TV", "onResume check flag exists=${flagFile.exists()} attached=$webViewAttached")
        if (flagFile.exists()) {
            flagFile.delete()
            pendingOpenSourcesFromIntent = true
        }

        // IIFE + try/catch por paso: un `const` global repetido o un fallo en updateFocus()
        // ya no impide que se ejecute crHandleReturnFromPlayer().
        val returnJs = """
            (function () {
              try { document.documentElement.classList.remove('playing-mode'); document.body.classList.remove('playing-mode'); } catch (e) {}
              try { var osd = document.getElementById('player-osd'); if (osd) osd.classList.add('hidden'); } catch (e) {}
              try { if (typeof isPlayerOpen !== 'undefined') isPlayerOpen = false; } catch (e) {}
              try { if (typeof updateFocus === 'function') updateFocus(); } catch (e) {}
              try { if (typeof window.crHandleReturnFromPlayer === 'function') window.crHandleReturnFromPlayer(); } catch (e) {}
            })();
        """.trimIndent()
        webViewRef?.evaluateJavascript(returnJs, null)
        // Reintento: si el flag de cambio de episodio sigue en disco, JS aún no lo consumió
        mainHandler.postDelayed({
            val f = java.io.File(applicationContext.filesDir, "play_ep.flag")
            if (f.exists()) {
                android.util.Log.d("CR_TV", "onResume retry: play_ep.flag sigue existiendo")
                webViewRef?.evaluateJavascript(returnJs, null)
            }
        }, 800L)
        webViewRef?.requestFocus()

        if (pendingOpenSourcesFromIntent && webViewAttached) {
            triggerOpenSourcesModal()
        }
        if (webViewAttached) {
            checkSubsFlagAndTrigger()
        }
    }


    override fun onDestroy() {
        mainHandler.removeCallbacksAndMessages(null)
        webViewRef = null
        super.onDestroy()
    }

      inner class ExoBridge {
        @JavascriptInterface
        fun consumeOpenSourcesFlag(): Boolean {
            val flagFile = java.io.File(applicationContext.filesDir, "open_sources.flag")
            if (flagFile.exists()) {
                flagFile.delete()
                pendingOpenSourcesFromIntent = false
                android.util.Log.d("CR_TV", "consumeOpenSourcesFlag -> TRUE (leído por JS)")
                return true
            }
            return false
        }

        /** Devuelve la posición (ms) guardada por el reproductor, o -1 si no hay flag. */
        @JavascriptInterface
        fun consumeOpenSubsFlag(): Long {
            val ms = takeSubsFlag()
            if (ms >= 0L) {
                pendingOpenSubsMs = -1L
                android.util.Log.d("CR_TV", "consumeOpenSubsFlag -> $ms (leído por JS)")
            }
            return ms
        }

        /** Guardado inmediato (con fsync) de pequeños JSON: el WebView puede perder localStorage si el proceso muere. */
        @JavascriptInterface
        fun writeKv(name: String, value: String): Boolean {
            return try {
                val safe = name.replace(Regex("[^a-zA-Z0-9_]"), "_")
                val dir = applicationContext.filesDir
                val tmp = java.io.File(dir, "kv_$safe.json.tmp")
                java.io.FileOutputStream(tmp).use { out ->
                    out.write(value.toByteArray(Charsets.UTF_8))
                    out.fd.sync()
                }
                tmp.renameTo(java.io.File(dir, "kv_$safe.json"))
            } catch (_: Exception) {
                false
            }
        }

        @JavascriptInterface
        fun readKv(name: String): String {
            return try {
                val safe = name.replace(Regex("[^a-zA-Z0-9_]"), "_")
                val f = java.io.File(applicationContext.filesDir, "kv_$safe.json")
                if (f.exists()) f.readText(Charsets.UTF_8) else ""
            } catch (_: Exception) {
                ""
            }
        }

        /** "animeId|episodio|posMs|durMs" guardado por el reproductor, o "" si no hay. */
        @JavascriptInterface
        fun consumeProgress(): String {
            val f = java.io.File(applicationContext.filesDir, "progress.txt")
            if (!f.exists()) return ""
            val raw = try { f.readText().trim() } catch (_: Exception) { "" }
            f.delete()
            return raw
        }

        /** Episodio pedido desde el reproductor (siguiente / lista), o 0 si no hay. */
        @JavascriptInterface
        fun consumePlayEpFlag(): Int = takeEpFlag()

        /** Mensajes del JS hacia logcat (etiqueta CR_TV) para diagnosticar. */
        @JavascriptInterface
        fun log(msg: String) {
            android.util.Log.d("CR_TV", "JS " + msg)
        }

        @JavascriptInterface
        fun playEpisode(
            url: String, subtitlePath: String?, title: String?, startMs: Long,
            epNum: Int, epMax: Int, animeKey: String?
        ) {
            mainHandler.post {
                val intent = Intent(this@MainActivity, PlayerActivity::class.java).apply {
                    putExtra("VIDEO_URL", url)
                    putExtra("SUBTITLE_PATH", subtitlePath)
                    putExtra("EPISODE_TITLE", title)
                    putExtra("START_MS", startMs)
                    putExtra("EP_NUM", epNum)
                    putExtra("EP_MAX", epMax)
                    putExtra("ANIME_KEY", animeKey ?: "")
                }
                startActivity(intent)
            }
        }

        @JavascriptInterface
        fun playWithTitle(url: String, subtitlePath: String?, title: String?) {
            playWithTitleAt(url, subtitlePath, title, 0L)
        }

        @JavascriptInterface
        fun playWithTitleAt(url: String, subtitlePath: String?, title: String?, startMs: Long) {
            mainHandler.post {
                val intent = Intent(this@MainActivity, PlayerActivity::class.java).apply {
                    putExtra("VIDEO_URL", url)
                    putExtra("SUBTITLE_PATH", subtitlePath)
                    putExtra("EPISODE_TITLE", title)
                    putExtra("START_MS", startMs)
                }
                startActivity(intent)
            }
        }

        @JavascriptInterface
        fun pauseToggle() {}

        @JavascriptInterface
        fun seekTo(seconds: Double) {}

        @JavascriptInterface
        fun seekRelative(offsetSeconds: Double) {}

        @JavascriptInterface
        fun stop() {}

        @JavascriptInterface
        fun setTrack(trackType: String, trackIndex: Int) {}

        @JavascriptInterface
        fun getStatusJson(): String {
            return """{"paused":false,"time_pos":0.0,"duration":0.0,"tracks":[]}"""
        }


        
    }
}