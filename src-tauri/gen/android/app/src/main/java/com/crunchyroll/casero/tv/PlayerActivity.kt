package com.crunchyroll.casero.tv

import android.content.res.ColorStateList
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.TextView
import androidx.annotation.OptIn
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.SeekParameters
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.CaptionStyleCompat
import androidx.media3.ui.PlayerView
import java.io.File

class PlayerActivity : AppCompatActivity() {
    private var exoPlayer: ExoPlayer? = null
    private var playerView: PlayerView? = null

    @OptIn(UnstableApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val insetsController = WindowCompat.getInsetsController(window, window.decorView)
        insetsController.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        insetsController.hide(WindowInsetsCompat.Type.systemBars())

        // Buffer optimizado: arranque rápido (1.2s) y colchón amplio (35s) para seeks fluidos
        val loadControl = DefaultLoadControl.Builder()
            .setBufferDurationsMs(
                15_000, // minBufferMs
                35_000, // maxBufferMs
                1_200,  // bufferForPlaybackMs (arranque rápido)
                2_500   // bufferForPlaybackAfterRebufferMs
            )
            .setPrioritizeTimeOverSizeThresholds(true)
            .build()

        // Decodificadores hardware nativos (Codec2 / MediaTek 4K) con fallback seguro.
        // Evita el crash FORTIFY pthread_mutex_lock de NextLib JNI en armeabi-v7a (Android 14).
        val renderersFactory = DefaultRenderersFactory(this)
            .setEnableDecoderFallback(true)
            .setExtensionRendererMode(DefaultRenderersFactory.EXTENSION_RENDERER_MODE_OFF)

        val httpDataSourceFactory = DefaultHttpDataSource.Factory()
            .setAllowCrossProtocolRedirects(true)
            .setConnectTimeoutMs(15_000)
            .setReadTimeoutMs(20_000)

        val dataSourceFactory = DefaultDataSource.Factory(this, httpDataSourceFactory)
        val mediaSourceFactory = DefaultMediaSourceFactory(dataSourceFactory)

        val player = ExoPlayer.Builder(this, renderersFactory)
            .setMediaSourceFactory(mediaSourceFactory)
            .setSeekBackIncrementMs(10_000)
            .setSeekForwardIncrementMs(10_000)
            .setLoadControl(loadControl)
            .build()

        // Seek rápido al keyframe más cercano para respuesta instantánea con el control remoto
        player.setSeekParameters(SeekParameters.CLOSEST_SYNC)

        player.trackSelectionParameters = player.trackSelectionParameters
            .buildUpon()
            .setPreferredAudioLanguages("ja", "jpn", "es-419", "es", "spa")
            .setPreferredTextLanguages("es-419", "es-MX", "es-LA", "spa", "es")
            .build()

        exoPlayer = player

        playerView = PlayerView(this).apply {
            this.player = exoPlayer
            useController = true
            controllerShowTimeoutMs = 3500
            controllerAutoShow = true
            setShowBuffering(PlayerView.SHOW_BUFFERING_WHEN_PLAYING)
            setBackgroundColor(Color.BLACK)
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
            subtitleView?.setStyle(
                CaptionStyleCompat(
                    Color.WHITE,
                    Color.TRANSPARENT,
                    Color.TRANSPARENT,
                    CaptionStyleCompat.EDGE_TYPE_OUTLINE,
                    Color.BLACK,
                    null
                )
            )
            subtitleView?.setFractionalTextSize(0.055f)
        }

        setContentView(playerView)

        val videoUrl = intent.getStringExtra("VIDEO_URL") ?: run {
            finish()
            return
        }
        val subtitlePath = intent.getStringExtra("SUBTITLE_PATH")
        val episodeTitle = intent.getStringExtra("EPISODE_TITLE")

        playerView?.post {
            val backBtn = findViewById<Button>(R.id.cr_back_btn)
            val tracksBtn = findViewById<Button>(R.id.cr_tracks_btn)
            val titleView = findViewById<TextView>(R.id.cr_episode_title)

            if (!episodeTitle.isNullOrBlank()) {
                titleView?.text = episodeTitle
            } else {
                val cleanName = Uri.parse(videoUrl).lastPathSegment
                    ?.substringBeforeLast('.')
                    ?.replace('.', ' ')
                    ?.replace('_', ' ')
                if (!cleanName.isNullOrBlank()) {
                    titleView?.text = cleanName
                }
            }

            backBtn?.setOnClickListener {
                finish()
            }
            tracksBtn?.setOnClickListener {
                showMainSettingsDialog()
            }

            val focusHighlight = View.OnFocusChangeListener { v, hasFocus ->
                v.backgroundTintList = ColorStateList.valueOf(
                    if (hasFocus) 0xFFF47521.toInt() else 0x26FFFFFF
                )
                (v as? Button)?.setTextColor(
                    if (hasFocus) Color.BLACK else Color.WHITE
                )
            }
            backBtn?.onFocusChangeListener = focusHighlight
            tracksBtn?.onFocusChangeListener = focusHighlight

            // Foco inicial en el botón central de Play/Pause o en el PlayerView para D-Pad fluido
            val playPauseBtn = findViewById<View>(androidx.media3.ui.R.id.exo_play_pause)
            if (playPauseBtn != null && playPauseBtn.isFocusable) {
                playPauseBtn.requestFocus()
            } else {
                playerView?.requestFocus()
            }
            playerView?.showController()
        }

        val itemBuilder = MediaItem.Builder().setUri(Uri.parse(videoUrl))
        if (!subtitlePath.isNullOrBlank()) {
            val subUri = if (subtitlePath.startsWith("http")) {
                Uri.parse(subtitlePath)
            } else {
                Uri.fromFile(File(subtitlePath))
            }
            val subConfig = MediaItem.SubtitleConfiguration.Builder(subUri)
                .setMimeType(MimeTypes.APPLICATION_SUBRIP)
                .setLanguage("es")
                .setLabel("Español Latino")
                .setSelectionFlags(C.SELECTION_FLAG_DEFAULT)
                .build()
            itemBuilder.setSubtitleConfigurations(listOf(subConfig))
        }

        var retriedWithoutText = false
        player.addListener(object : Player.Listener {
            override fun onPlayerError(error: PlaybackException) {
                Log.e("PlayerActivity", "ExoPlayer error: ${error.errorCodeName}", error)
                if (!retriedWithoutText) {
                    retriedWithoutText = true
                    // Ejecución asíncrona segura para que los hilos nativos liberen recursos antes de reintentar
                    playerView?.post {
                        exoPlayer?.let { p ->
                            p.stop()
                            p.trackSelectionParameters = p.trackSelectionParameters
                                .buildUpon()
                                .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
                                .build()
                            p.prepare()
                            p.playWhenReady = true
                        }
                    }
                }
            }
        })

        exoPlayer?.setMediaItem(itemBuilder.build())
        exoPlayer?.prepare()
        exoPlayer?.playWhenReady = true
    }

    private fun formatLanguageName(langCode: String?, rawLabel: String?, index: Int): String {
        val code = (langCode ?: "").lowercase()
        val label = rawLabel ?: ""
        return when {
            code.startsWith("ja") || code == "jpn" -> "Japonés"
            code == "es-419" || code == "es-mx" || code == "es-la" || label.contains("latino", true) -> "Español (América Latina)"
            code.startsWith("es") || code == "spa" -> "Español"
            code.startsWith("en") || code == "eng" -> "Inglés"
            code.startsWith("pt") || code == "por" -> "Portugués"
            label.isNotBlank() -> label
            else -> "Pista ${index + 1}"
        }
    }

    private fun showMainSettingsDialog() {
        val menuItems = arrayOf(
            "🔊  Audio",
            "💬  Subtítulos / CC"
        )
        AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle("Configuración de reproducción")
            .setItems(menuItems) { _, which ->
                if (which == 0) {
                    showTrackCategoryDialog(C.TRACK_TYPE_AUDIO, "Seleccionar Audio")
} else {
showTrackCategoryDialog(C.TRACK_TYPE_TEXT, "Seleccionar Subtítulos")
}
}
.show()
}
private fun showTrackCategoryDialog(trackType: Int, dialogTitle: String) {
val player = exoPlayer ?: return
val options = mutableListOf<String>()
val actions = mutableListOf<() -> Unit>()
if (trackType == C.TRACK_TYPE_TEXT) {
val isSubDisabled = player.trackSelectionParameters.disabledTrackTypes.contains(C.TRACK_TYPE_TEXT)
options.add(if (isSubDisabled) "Desactivado  ✓" else "Desactivado")
actions.add {
player.trackSelectionParameters = player.trackSelectionParameters
.buildUpon()
.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
.build()
}
}
var idx = 0
for (group in player.currentTracks.groups) {
if (group.type == trackType) {
for (i in 0 until group.length) {
val fmt = group.getTrackFormat(i)
val friendlyName = formatLanguageName(fmt.language, fmt.label, idx)
val isSelected = group.isTrackSelected(i) &&
!player.trackSelectionParameters.disabledTrackTypes.contains(trackType)
options.add(if (isSelected) "$friendlyName  ✓" else friendlyName)
actions.add {
player.trackSelectionParameters = player.trackSelectionParameters
.buildUpon()
.setTrackTypeDisabled(trackType, false)
.setOverrideForType(TrackSelectionOverride(group.mediaTrackGroup, i))
.build()
}
idx++
}
}
}
AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
.setTitle(dialogTitle)
.setItems(options.toTypedArray()) { _, which ->
actions[which].invoke()
}
.setNegativeButton("‹ Volver") { _, _ ->
showMainSettingsDialog()
}
.show()
}
@OptIn(UnstableApi::class)
override fun dispatchKeyEvent(event: KeyEvent): Boolean {
val player = exoPlayer
val pv = playerView
if (event.action == KeyEvent.ACTION_DOWN && player != null && pv != null) {
val isControllerVisible = pv.isControllerFullyVisible
// Cálculo progresivo si el usuario deja presionada la flecha izquierda/derecha
val stepMs = when {
event.repeatCount >= 10 -> 30_000L
event.repeatCount >= 4 -> 20_000L
else -> 10_000L
}
when (event.keyCode) {
 KeyEvent.KEYCODE_BACK -> {
                    // Si el menú de controles está abierto, primero lo oculta; si ya está oculto, sale al catálogo
                    if (isControllerVisible && event.repeatCount == 0) {
                        pv.hideController()
                    } else {
                        finish()
                    }
                    return true
                }

                KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> {
                    if (!isControllerVisible || event.keyCode == KeyEvent.KEYCODE_MEDIA_FAST_FORWARD) {
                        val maxPos: Long = if (player.duration > 0L) player.duration else Long.MAX_VALUE
                        val target: Long = kotlin.math.min(player.currentPosition + stepMs, maxPos)
                        player.seekTo(target)
                        pv.showController()
                        return true
                    }
                }

                KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_MEDIA_REWIND -> {
                    if (!isControllerVisible || event.keyCode == KeyEvent.KEYCODE_MEDIA_REWIND) {
                        val target: Long = kotlin.math.max(player.currentPosition - stepMs, 0L)
                        player.seekTo(target)
                        pv.showController()
                        return true
                    }
                }

                KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER -> {
                    if (!isControllerVisible) {
                        if (player.isPlaying) player.pause() else player.play()
                        pv.showController()
                        return true
                    }
                }

                KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE -> {
                    if (player.isPlaying) player.pause() else player.play()
                    pv.showController()
                    return true
                }

                KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN -> {
                    if (!isControllerVisible) {
                        pv.showController()
                        return true
                    }
                }
            }
        }

        return super.dispatchKeyEvent(event)
    }

    override fun onPause() {
        super.onPause()
        exoPlayer?.pause()
    }

    override fun onDestroy() {
        super.onDestroy()
        exoPlayer?.release()
        exoPlayer = null
    }
}