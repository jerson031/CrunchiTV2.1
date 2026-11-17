package com.crunchyroll.casero.tv

import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
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

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val insetsController = WindowCompat.getInsetsController(window, window.decorView)
        insetsController.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        insetsController.hide(WindowInsetsCompat.Type.systemBars())

        mainHandler.post {
            val rootContent = findViewById<ViewGroup>(android.R.id.content)
            if (rootContent != null) {
                attachWebViewBridge(rootContent)
            }
        }
    }

    private fun attachWebViewBridge(root: ViewGroup) {
        mainHandler.postDelayed(object : Runnable {
            override fun run() {
                if (webViewAttached) return
                val wv = findWebView(root)
                if (wv != null) {
                    webViewRef = wv
                    wv.addJavascriptInterface(ExoBridge(), "AndroidExo")
                    wv.isFocusable = true
                    wv.isFocusableInTouchMode = true
                    wv.requestFocus()
                    webViewAttached = true
                } else {
                    mainHandler.postDelayed(this, 150)
                }
            }
        }, 150)
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
        if (event.keyCode == KeyEvent.KEYCODE_BACK && event.action == KeyEvent.ACTION_DOWN) {
            webViewRef?.evaluateJavascript(
                "window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape' }));",
                null
            )
            return true
        }
        return super.dispatchKeyEvent(event)
    }

 override fun onResume() {
        super.onResume()
        webViewRef?.evaluateJavascript(
            """
            document.documentElement.classList.remove('playing-mode');
            document.body.classList.remove('playing-mode');
            const osd = document.getElementById('player-osd');
            if (osd) osd.classList.add('hidden');
            if (typeof isPlayerOpen !== 'undefined') isPlayerOpen = false;
            """.trimIndent(),
            null
        )
        webViewRef?.requestFocus()
    }



    inner class ExoBridge {
        @JavascriptInterface
        fun playWithTitle(url: String, subtitlePath: String?, title: String?) {
            mainHandler.post {
                val intent = Intent(this@MainActivity, PlayerActivity::class.java).apply {
                    putExtra("VIDEO_URL", url)
                    putExtra("SUBTITLE_PATH", subtitlePath)
                    putExtra("EPISODE_TITLE", title)
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
