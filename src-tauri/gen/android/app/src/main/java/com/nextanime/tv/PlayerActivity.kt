package com.nextanime.tv
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import androidx.annotation.OptIn
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
import io.github.anilbeesetti.nextlib.media3ext.ffdecoder.NextRenderersFactory
import io.github.anilbeesetti.nextlib.media3ext.ffdecoder.FfmpegSubtitleExtractorsFactory
import io.github.anilbeesetti.nextlib.media3ext.ffdecoder.withFfmpegSubtitles
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.SeekParameters
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.CaptionStyleCompat
import androidx.media3.ui.PlayerView
import java.io.File

class PlayerActivity : AppCompatActivity() {
    private var exoPlayer: ExoPlayer? = null
    private var playerView: PlayerView? = null
    private var settingsOverlay: FrameLayout? = null
    private var rootLayout: FrameLayout? = null
    private var episodesTab: TextView? = null

    // Episodio en reproducción (para "siguiente", lista de episodios y guardar el minuto)
    private var epNum = 0
    private var epMax = 0
    // Salida protegida: Atrás dos veces seguidas, y se ignora justo después de cerrar un menú
    private var backArmedAt = 0L
    private var overlayClosedAt = 0L
    private var animeKey = ""
    private val progressHandler = Handler(Looper.getMainLooper())
    private val progressRunnable = object : Runnable {
        override fun run() {
            saveProgress()
            progressHandler.postDelayed(this, 5000L)
        }
    }
    // Auto-ocultar barras: 4 s sin tocar nada mientras se reproduce
    private val hideRunnable = Runnable {
        if (settingsOverlay?.visibility != View.VISIBLE && exoPlayer?.isPlaying == true) {
            playerView?.hideController()
        }
    }
    private fun scheduleHide() {
        progressHandler.removeCallbacks(hideRunnable)
        progressHandler.postDelayed(hideRunnable, 4000L)
    }
    private var overlayTitleView: TextView? = null
    private var overlayItemsContainer: LinearLayout? = null

    @OptIn(UnstableApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val insetsController = WindowCompat.getInsetsController(window, window.decorView)
        insetsController.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        insetsController.hide(WindowInsetsCompat.Type.systemBars())

        val loadControl = DefaultLoadControl.Builder()
            .setBufferDurationsMs(
                20_000,
                45_000,
                1_000,
                3_000
            )
            .setPrioritizeTimeOverSizeThresholds(true)
            .build()

        // NextRenderersFactory = DefaultRenderersFactory + decodificadores FFmpeg (audio: DTS/E-AC3/TrueHD...,
        // video: HEVC/AV1/VP9...). Con EXTENSION_RENDERER_MODE_ON el hardware del equipo va primero y FFmpeg
        // solo entra cuando el sistema no puede decodificar el formato.
        val renderersFactory = NextRenderersFactory(this)
            .setEnableDecoderFallback(true)
            .forceEnableMediaCodecAsynchronousQueueing()
            .setExtensionRendererMode(DefaultRenderersFactory.EXTENSION_RENDERER_MODE_ON)

        val httpDataSourceFactory = DefaultHttpDataSource.Factory()
            .setAllowCrossProtocolRedirects(true)
            .setConnectTimeoutMs(15_000)
            .setReadTimeoutMs(20_000)

        val dataSourceFactory = DefaultDataSource.Factory(this, httpDataSourceFactory)
        // Subtítulos ASS/SSA con estilo (libass de NextLib). Si algo falla, el reintento de onPlayerError
        // usa safeMediaSourceFactory (sin libass), así que el comportamiento anterior queda como respaldo.
        val normalMediaSourceFactory = DefaultMediaSourceFactory(this, FfmpegSubtitleExtractorsFactory())
            .setDataSourceFactory(dataSourceFactory)
            .withFfmpegSubtitles(dataSourceFactory)
        val safeMediaSourceFactory = DefaultMediaSourceFactory(dataSourceFactory)
            .experimentalParseSubtitlesDuringExtraction(false)

        val player = ExoPlayer.Builder(this, renderersFactory)
            .setMediaSourceFactory(normalMediaSourceFactory)
            .setSeekBackIncrementMs(10_000)
            .setSeekForwardIncrementMs(10_000)
            .setLoadControl(loadControl)
            .build()

        player.setSeekParameters(SeekParameters.CLOSEST_SYNC)

        player.trackSelectionParameters = player.trackSelectionParameters
            .buildUpon()
            .setPreferredAudioLanguages("ja", "jpn", "es-419", "es", "spa")
            .setPreferredTextLanguages("es-419", "es-MX", "es-LA", "spa", "es")
            .build()

        exoPlayer = player

        val rootContainer = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }

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

        rootLayout = rootContainer
        rootContainer.addView(playerView)
        playerView?.setControllerVisibilityListener(PlayerView.ControllerVisibilityListener { vis ->
            val shown = vis == View.VISIBLE
            val canEp = epNum > 0 && epMax != 1
            episodesTab?.visibility = if (shown && canEp) View.VISIBLE else View.GONE
            if (shown) scheduleHide() else progressHandler.removeCallbacks(hideRunnable)
        })
        buildInternalSettingsOverlay(rootContainer)
        setContentView(rootContainer)


        val videoUrl = intent.getStringExtra("VIDEO_URL") ?: run {
            finish()
            return
        }
        val subtitlePath = intent.getStringExtra("SUBTITLE_PATH")
        val episodeTitle = intent.getStringExtra("EPISODE_TITLE")
        val startMs = intent.getLongExtra("START_MS", 0L)
        epNum = intent.getIntExtra("EP_NUM", 0)
        epMax = intent.getIntExtra("EP_MAX", 0)
        animeKey = intent.getStringExtra("ANIME_KEY") ?: ""

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

            val playPauseBtn = findViewById<View>(androidx.media3.ui.R.id.exo_play_pause)
            if (playPauseBtn != null && playPauseBtn.isFocusable) {
                playPauseBtn.requestFocus()
            } else {
                playerView?.requestFocus()
            }
            installNextButton()
            installEpisodesTab()
            playerView?.showController()
            scheduleHide()
        }

        val itemBuilder = MediaItem.Builder().setUri(Uri.parse(videoUrl))
        // Solo usamos el subtítulo externo si es una URL o si el archivo realmente existe
        val hasExternalSub = !subtitlePath.isNullOrBlank() &&
            (subtitlePath.startsWith("http") || File(subtitlePath).exists())
        if (hasExternalSub && !subtitlePath.isNullOrBlank()) {
            val subUri = if (subtitlePath.startsWith("http")) {
                Uri.parse(subtitlePath)
            } else {
                Uri.fromFile(File(subtitlePath))
            }
            val subConfig = MediaItem.SubtitleConfiguration.Builder(subUri)
                .setMimeType(subtitleMimeFor(subtitlePath))
                .setLanguage("es")
                .setLabel("Español Latino")
                .setSelectionFlags(C.SELECTION_FLAG_DEFAULT)
                .build()
            itemBuilder.setSubtitleConfigurations(listOf(subConfig))
        }

            val mediaItem = itemBuilder.build()
        var retriedSafeExtraction = false
        var retriedWithoutExternalSub = false
        player.addListener(object : Player.Listener {
            override fun onPlayerError(error: PlaybackException) {
                Log.e("PlayerActivity", "ExoPlayer error: ${error.errorCodeName}", error)
                if (!retriedSafeExtraction) {
                    retriedSafeExtraction = true
                    Log.w("PlayerActivity", "Reintentando reproducción sin parser ASS interno...")
                    playerView?.post {
                        exoPlayer?.let { p ->
                            val safeSource = safeMediaSourceFactory.createMediaSource(mediaItem)
                            p.stop()
                            p.clearMediaItems()
                            playerView?.player = null
                            playerView?.player = p
                            p.setMediaSource(safeSource)
                            p.prepare()
                            p.playWhenReady = true
                        }
                    }

                } else if (hasExternalSub && !retriedWithoutExternalSub) {
                    // Segundo fallback: si lo que falla es el subtítulo externo, reproducimos sin él
                    retriedWithoutExternalSub = true
                    Log.w("PlayerActivity", "Reintentando reproducción SIN subtítulo externo...")
                    playerView?.post {
                        exoPlayer?.let { p ->
                            val noSubItem = mediaItem.buildUpon()
                                .setSubtitleConfigurations(emptyList())
                                .build()
                            p.stop()
                            p.clearMediaItems()
                            p.setMediaSource(safeMediaSourceFactory.createMediaSource(noSubItem))
                            p.prepare()
                            p.playWhenReady = true
                        }
                    }
                }
            }
        })

        player.addListener(object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                if (isPlaying && playerView?.isControllerFullyVisible == true) scheduleHide()
            }
        })

        if (startMs > 0L) exoPlayer?.setMediaItem(mediaItem, startMs) else exoPlayer?.setMediaItem(mediaItem)
        exoPlayer?.prepare()
        exoPlayer?.playWhenReady = true
        progressHandler.postDelayed(progressRunnable, 5000L)

    }

    /** El MIME depende de la extensión: un .ass leído como SubRip sale vacío o roto. */
    private fun subtitleMimeFor(path: String): String {
        val lower = path.lowercase().substringBefore('?')
        return when {
            lower.endsWith(".ass") || lower.endsWith(".ssa") -> MimeTypes.TEXT_SSA
            lower.endsWith(".vtt") -> MimeTypes.TEXT_VTT
            else -> MimeTypes.APPLICATION_SUBRIP
        }
    }

    private fun formatLanguageName(langCode: String?, rawLabel: String?, index: Int): String {
        val code = (langCode ?: "").lowercase()
        val label = (rawLabel ?: "").trim()
        val base = when {
            code.startsWith("ja") || code == "jpn" -> "Japonés"
            code == "es-419" || code == "es-mx" || code == "es-la" || label.contains("latino", true) -> "Español (América Latina)"
            code.startsWith("es") || code == "spa" -> "Español"
            code.startsWith("en") || code == "eng" -> "Inglés"
            code.startsWith("pt") || code == "por" -> "Portugués"
            label.isNotBlank() -> label
            else -> "Pista ${index + 1}"
        }
        return if (label.isNotBlank() && !base.equals(label, ignoreCase = true)) {
            "$base ($label)"
        } else {
            base
        }
    }

       private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()

    private fun buildInternalSettingsOverlay(root: FrameLayout) {
        val backdrop = FrameLayout(this).apply {
            // Degradado: la izquierda queda libre para ver el video, el panel va a la derecha
            background = GradientDrawable(
                GradientDrawable.Orientation.LEFT_RIGHT,
                intArrayOf(0x33000000, 0xB3000000.toInt())
            )
            visibility = View.GONE
            isClickable = true
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
            setOnClickListener { hideSettingsOverlay() }
        }

        val cardBg = GradientDrawable().apply {
            setColor(0xF2111218.toInt())
            cornerRadius = dp(20).toFloat()
            setStroke(dp(1), 0x1FFFFFFF)
        }

        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = cardBg
            setPadding(dp(20), dp(20), dp(20), dp(16))
            isClickable = true
            layoutParams = FrameLayout.LayoutParams(
                dp(400),
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.END or Gravity.CENTER_VERTICAL
            ).apply {
                marginEnd = dp(32)
                topMargin = dp(24)
                bottomMargin = dp(24)
            }
        }

        val titleTv = TextView(this).apply {
            text = "Configuración de reproducción"
            setTextColor(Color.WHITE)
            textSize = 18f
            setTypeface(typeface, Typeface.BOLD)
            setPadding(dp(6), 0, dp(6), dp(14))
        }

        val scroll = ScrollView(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        val listContainer = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }

        scroll.addView(listContainer)
        card.addView(titleTv)
        card.addView(scroll)
        backdrop.addView(card)
        root.addView(backdrop)

        settingsOverlay = backdrop
        overlayTitleView = titleTv
        overlayItemsContainer = listContainer
    }

    private fun hideSettingsOverlay() {
        overlayClosedAt = SystemClock.elapsedRealtime()
        settingsOverlay?.visibility = View.GONE
        playerView?.showController()
        val tracksBtn = findViewById<Button>(R.id.cr_tracks_btn)
        if (tracksBtn != null && tracksBtn.isFocusable) {
            tracksBtn.requestFocus()
        } else {
            playerView?.requestFocus()
        }
    }

    private fun renderOverlayMenu(title: String, items: List<Pair<String, () -> Unit>>) {
        val overlay = settingsOverlay ?: return
        val container = overlayItemsContainer ?: return
        overlayTitleView?.text = title
        container.removeAllViews()

        items.forEachIndexed { idx, (label, onClick) ->
            val normalBg = GradientDrawable().apply {
                setColor(0x14FFFFFF)
                cornerRadius = dp(12).toFloat()
                setStroke(dp(2), Color.TRANSPARENT)
            }
            // Foco: fondo naranja tenue + borde naranja (texto blanco, siempre legible)
            val focusedBg = GradientDrawable().apply {
                setColor(0x38F47521)
                cornerRadius = dp(12).toFloat()
                setStroke(dp(2), 0xFFF47521.toInt())
            }

            val btn = Button(this).apply {
                text = label
                isAllCaps = false
                textSize = 15f
                maxLines = 2
                stateListAnimator = null
                gravity = Gravity.START or Gravity.CENTER_VERTICAL
                setPadding(dp(16), dp(12), dp(16), dp(12))
                setTextColor(Color.WHITE)
                background = normalBg
                isFocusable = true
                isFocusableInTouchMode = true
                layoutParams = LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT
                ).apply {
                    bottomMargin = dp(8)
                }
                setOnFocusChangeListener { _, hasFocus ->
                    background = if (hasFocus) focusedBg else normalBg
                    setTextColor(Color.WHITE)
                }
                setOnClickListener { onClick() }
            }
            container.addView(btn)
            if (idx == 0) {
                btn.post { btn.requestFocus() }
            }
        }

        playerView?.hideController()
        overlay.visibility = View.VISIBLE
    }

    private fun roundedBg(fill: Int, stroke: Int): GradientDrawable = GradientDrawable().apply {
        setColor(fill)
        cornerRadius = dp(24).toFloat()
        setStroke(dp(2), stroke)
    }

    /** Resalta con naranja el botón que tiene el foco (control remoto). */
    private fun styleFocus(v: View?) {
        if (v == null) return
        val normal = roundedBg(Color.TRANSPARENT, Color.TRANSPARENT)
        val focused = roundedBg(0x38F47521, 0xFFF47521.toInt())
        v.background = normal
        v.setOnFocusChangeListener { view, f -> view.background = if (f) focused else normal }
    }

    /** Botón "siguiente episodio" (definido en el XML, a la derecha de adelantar). */
    private fun installNextButton() {
        val btn = findViewById<View>(R.id.cr_next_btn)
        styleFocus(findViewById(androidx.media3.ui.R.id.exo_rew))
        styleFocus(findViewById(androidx.media3.ui.R.id.exo_play_pause))
        styleFocus(findViewById(androidx.media3.ui.R.id.exo_ffwd))
        if (btn == null || !hasNextEpisode()) return
        styleFocus(btn)
        btn.visibility = View.VISIBLE
        btn.setOnClickListener { requestEpisode(epNum + 1) }
    }

    /** Pestaña pequeña en el borde derecho (‹) que abre la lista de episodios. */
    private fun installEpisodesTab() {
        if (!(epNum > 0 && epMax != 1)) return
        val normal = GradientDrawable().apply {
            setColor(0xCC1B1B3A.toInt())
            cornerRadii = floatArrayOf(dp(22).toFloat(), dp(22).toFloat(), 0f, 0f, 0f, 0f, dp(22).toFloat(), dp(22).toFloat())
        }
        val focused = GradientDrawable().apply {
            setColor(0xCCF47521.toInt())
            cornerRadii = floatArrayOf(dp(22).toFloat(), dp(22).toFloat(), 0f, 0f, 0f, 0f, dp(22).toFloat(), dp(22).toFloat())
        }
        val tab = TextView(this).apply {
            text = "‹"
            textSize = 26f
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
            background = normal
            isFocusable = true
            visibility = View.GONE
            setOnFocusChangeListener { v, f -> v.background = if (f) focused else normal }
            setOnClickListener { showEpisodesDialog() }
        }
        episodesTab = tab
        rootLayout?.addView(tab, FrameLayout.LayoutParams(dp(30), dp(96), Gravity.END or Gravity.CENTER_VERTICAL))
        if (playerView?.isControllerFullyVisible == true) tab.visibility = View.VISIBLE
    }

    /** Guarda "animeId|episodio|posiciónMs|duraciónMs" para reanudar y para "Continuar viendo". */
    private fun saveProgress() {
        val p = exoPlayer ?: return
        if (animeKey.isBlank() || epNum <= 0) return
        val pos = p.currentPosition
        if (pos <= 0L) return
        val dur = if (p.duration > 0L) p.duration else 0L
        try {
            File(applicationContext.filesDir, "progress.txt").writeText("$animeKey|$epNum|$pos|$dur")
        } catch (_: Exception) {}
    }

    private fun hasNextEpisode(): Boolean = epNum > 0 && (epMax == 0 || epNum < epMax)

    /** Cierra el reproductor y pide a la app abrir otro episodio (sin pasar por el catálogo). */
    private fun requestEpisode(ep: Int) {
        android.util.Log.d("CR_TV", "PLAYER requestEpisode ep=$ep (viendo=$epNum max=$epMax key=$animeKey)")
        saveProgress()
        settingsOverlay?.visibility = View.GONE
        try {
            File(applicationContext.filesDir, "play_ep.flag").writeText(ep.toString())
        } catch (_: Exception) {}
        finish()
    }

    /** Ventana de 10 episodios alrededor del actual (sin repetir el actual). */
    private fun showEpisodesDialog() {
        val max = if (epMax > 0) epMax else epNum + 9
        val start = kotlin.math.max(1, kotlin.math.min(epNum - 4, max - 9))
        val end = kotlin.math.min(max, start + 9)
        val items = mutableListOf<Pair<String, () -> Unit>>()
        for (n in start..end) {
            if (n == epNum) {
                items.add("▶  Episodio $n  ·  viendo" to { hideSettingsOverlay() })
            } else {
                items.add("Episodio $n" to { requestEpisode(n) })
            }
        }
        items.add("✕  Cerrar" to { hideSettingsOverlay() })
        renderOverlayMenu("Episodios", items)
    }

    private fun showMainSettingsDialog() {
        val items = mutableListOf<Pair<String, () -> Unit>>()
        items.add("🔊  Audio" to { showTrackCategoryDialog(C.TRACK_TYPE_AUDIO, "Seleccionar Audio") })
        items.add("💬  Subtítulos / CC" to { showTrackCategoryDialog(C.TRACK_TYPE_TEXT, "Seleccionar Subtítulos") })
        items.add("🎥  Cambiar fuente / release" to {
            saveProgress()
            settingsOverlay?.visibility = View.GONE
            try {
                File(applicationContext.filesDir, "open_sources.flag").writeText("1")
            } catch (_: Exception) {}
            finish()
        })
        items.add("🔎  Buscar otro subtítulo (otros proveedores)" to {
            settingsOverlay?.visibility = View.GONE
            try {
                val pos = exoPlayer?.currentPosition ?: 0L
                File(applicationContext.filesDir, "open_subs.flag").writeText(pos.toString())
            } catch (_: Exception) {}
            saveProgress()
            finish()
        })
        items.add("✕  Cerrar" to { hideSettingsOverlay() })
        renderOverlayMenu("Configuración de reproducción", items)
    }

    private fun showTrackCategoryDialog(trackType: Int, dialogTitle: String) {
        val player = exoPlayer ?: return
        val items = mutableListOf<Pair<String, () -> Unit>>()

        if (trackType == C.TRACK_TYPE_TEXT) {
            val isSubDisabled = player.trackSelectionParameters.disabledTrackTypes.contains(C.TRACK_TYPE_TEXT)
            val label = if (isSubDisabled) "Desactivado  ✓" else "Desactivado"
            items.add(label to {
                player.trackSelectionParameters = player.trackSelectionParameters
                    .buildUpon()
                    .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
                    .build()
                hideSettingsOverlay()
            })
        }

        var idx = 0
        for (group in player.currentTracks.groups) {
            if (group.type == trackType) {
                for (i in 0 until group.length) {
                    val fmt = group.getTrackFormat(i)
                    val friendlyName = formatLanguageName(fmt.language, fmt.label, idx)
                    val isSelected = group.isTrackSelected(i) &&
                        !player.trackSelectionParameters.disabledTrackTypes.contains(trackType)
                    val label = if (isSelected) "$friendlyName  ✓" else friendlyName
                    items.add(label to {
                        player.trackSelectionParameters = player.trackSelectionParameters
                            .buildUpon()
                            .setTrackTypeDisabled(trackType, false)
                            .setOverrideForType(TrackSelectionOverride(group.mediaTrackGroup, i))
                            .build()
                        hideSettingsOverlay()
                    })
                    idx++
                }
            }
        }

        items.add("‹  Volver" to { showMainSettingsDialog() })
        renderOverlayMenu(dialogTitle, items)
    }

    @OptIn(UnstableApi::class)
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (settingsOverlay?.visibility == View.VISIBLE) {
            if (event.action == KeyEvent.ACTION_DOWN && event.keyCode == KeyEvent.KEYCODE_BACK) {
                hideSettingsOverlay()
                return true
            }
            return super.dispatchKeyEvent(event)
        }

        val player = exoPlayer
        val pv = playerView
        if (event.action == KeyEvent.ACTION_DOWN && player != null && pv != null) {
            val isControllerVisible = pv.isControllerFullyVisible
            if (isControllerVisible) scheduleHide()
            val tab = episodesTab
            if (event.keyCode == KeyEvent.KEYCODE_MENU && epNum > 0 && epMax != 1) {
                showEpisodesDialog()
                return true
            }
            // → desde el borde derecho de los controles: salta a la pestaña de episodios
            if (isControllerVisible && tab != null && tab.visibility == View.VISIBLE &&
                event.keyCode == KeyEvent.KEYCODE_DPAD_RIGHT) {
                val f = currentFocus
                val isBar = f != null && f.id == androidx.media3.ui.R.id.exo_progress
                if (f != null && f !== tab && !isBar && f.focusSearch(View.FOCUS_RIGHT) == null) {
                    tab.requestFocus()
                    return true
                }
                if (f === tab) return true
            }
            if (isControllerVisible && tab != null && currentFocus === tab &&
                event.keyCode == KeyEvent.KEYCODE_DPAD_LEFT) {
                findViewById<View>(androidx.media3.ui.R.id.exo_play_pause)?.requestFocus()
                return true
            }
            val stepMs = when {
                event.repeatCount >= 10 -> 30_000L
                event.repeatCount >= 4 -> 20_000L
                else -> 10_000L
            }
            when (event.keyCode) {
                KeyEvent.KEYCODE_BACK -> {
                    // mantener presionado (repetición) nunca sale
                    if (event.repeatCount > 0) return true
                    val now = SystemClock.elapsedRealtime()
                    if (isControllerVisible) {
                        pv.hideController()
                    } else if (now - overlayClosedAt < 800L) {
                        // el Atrás que cerró el menú no debe contar como salida
                    } else if (now - backArmedAt < 2500L) {
                        finish()
                    } else {
                        backArmedAt = now
                        Toast.makeText(this, "Presiona Atrás otra vez para salir", Toast.LENGTH_SHORT).show()
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
                KeyEvent.KEYCODE_MEDIA_NEXT -> {
                    if (hasNextEpisode()) {
                        requestEpisode(epNum + 1)
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
saveProgress()
exoPlayer?.pause()
}
override fun onDestroy() {
    android.util.Log.d("CR_TV", "PLAYER onDestroy isFinishing=$isFinishing ep=$epNum")
progressHandler.removeCallbacks(progressRunnable)
progressHandler.removeCallbacks(hideRunnable)
saveProgress()
super.onDestroy()
exoPlayer?.release()
exoPlayer = null
}
}