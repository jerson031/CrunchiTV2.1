const { invoke } = window.__TAURI__.core;

let ANIMES = [];
let AIRING_CATALOG = [];
let TODAY_ANIMES = [];
let activeTab = "inicio";
let previousTabBeforeDetail = "inicio";
let savedCardIndexBeforeDetail = 0;

// Áreas de foco D-Pad:
// "sidebar" | "play" | "cards" | "filters" | "episodes" | "search_bar" | "detail"
let currentArea = "cards";
let cardIndex = 0;
let sideIndex = 0;
let filterIndex = 0;
let episodeIndex = 0;
let searchBarIndex = 0; // 0 = input, 1 = botón Buscar

// Sub-estado de navegación 2D dentro de #view-detail:
// "back" | "actions" | "ranges" | "grid"
let detailSection = "actions";
let detailActionIndex = 0;
let detailRangeIndex = 0;
let detailGridIndex = 0;

let isSearching = false;
let isPlayerOpen = false;

let activeAnime = null;
let activeEpisodeNum = 1;
let totalSeasonEpisodes = 1;
let currentTracks = [];
let currentDuration = 0;

let loadDelayTimer = null;
let loadSecondsInterval = null;
let osdPollInterval = null;
let osdHideTimer = null;
let currentPlayAttemptId = 0;

function formatTime(secs) {
  if (!secs || isNaN(secs) || secs < 0) return "0:00";
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return m + ":" + (s < 10 ? "0" : "") + s;
}

/* =====================================================================
   MODAL DE AVISO / ERROR PARA TV (REEMPLAZA A ALERT())
   ===================================================================== */
function showTvAlert(title, message) {
  const modal = document.getElementById("tv-alert-modal");
  if (!modal) return;
  document.getElementById("tv-alert-title").textContent = title || "Aviso";
  document.getElementById("tv-alert-message").textContent = message || "";
  modal.classList.remove("hidden");
  setTimeout(() => {
    document.getElementById("tv-alert-ok-btn")?.focus();
  }, 30);
}

function hideTvAlert() {
  const modal = document.getElementById("tv-alert-modal");
  if (!modal || modal.classList.contains("hidden")) return false;
  modal.classList.add("hidden");
  updateFocus();
  return true;
}

/* =====================================================================
   PANTALLA DE CARGA CON CANCELACIÓN SEGURA
   ===================================================================== */
function showLoadingEvent(animeOrTitle, epNum) {
  const aiToast = document.getElementById("ai-sub-toast");
  if (aiToast) {
    aiToast.classList.add("hidden");
    aiToast.style.display = "";
  }
  const isObj = typeof animeOrTitle === "object" && animeOrTitle !== null;
  const title = isObj ? animeOrTitle.title : animeOrTitle;
  const coverUrl = isObj ? animeOrTitle.cover : "";
  const bannerUrl = isObj ? (animeOrTitle.banner || animeOrTitle.cover) : "";

  let overlay = document.getElementById("loading-overlay");
  if (!overlay) {
    overlay = document.createElement("div");
    overlay.id = "loading-overlay";
    overlay.style.cssText =
      "position: fixed; inset: 0; background: #0b0b0d; overflow: hidden;" +
      "display: none; flex-direction: column; align-items: center; justify-content: center;" +
      "z-index: 2000; color: white; font-family: sans-serif;";

    overlay.innerHTML =
      '<div id="load-bg-blur" style="position:absolute; inset:0; background-size:cover; background-position:center; opacity:0.22; z-index:0;"></div>' +
      '<div style="position:relative; z-index:1; display:flex; flex-direction:column; align-items:center; text-align:center; padding:24px; width:100%; max-width:540px;">' +
      '<img id="load-cover-img" src="" alt="" style="width:155px; height:225px; object-fit:cover; border-radius:10px; box-shadow:0 12px 35px rgba(0,0,0,0.85); border:2px solid rgba(244,117,33,0.65); margin-bottom:20px; display:none;" />' +
      '<div style="width:42px; height:42px; border:4px solid rgba(255,255,255,0.18); border-top-color:#f47521; border-radius:50%; animation: spin 0.9s linear infinite;"></div>' +
      '<h2 id="load-title" style="margin-top:16px; font-size:22px; text-shadow:0 2px 10px rgba(0,0,0,0.9);">Preparando episodio...</h2>' +
      '<p id="load-status" style="color:#e0e0e0; margin-top:8px; font-size:15px; font-weight:500;">Conectando con TorBox (Español Latino)...</p>' +
      '<div id="load-progress-wrap" style="width:100%; margin-top:16px; display:none;">' +
      '<div style="display:flex; justify-content:space-between; font-size:13px; font-weight:700; color:#f47521; margin-bottom:6px;">' +
      '<span id="load-progress-step">Procesando...</span>' +
      '<span id="load-progress-pct">0%</span>' +
      '</div>' +
      '<div style="width:100%; height:8px; background:rgba(255,255,255,0.15); border-radius:6px; overflow:hidden;">' +
      '<div id="load-progress-bar" style="width:0%; height:100%; background:#f47521; border-radius:6px; transition:width 0.25s ease;"></div>' +
      '</div>' +
      '</div>' +
      '<button id="load-cancel-btn" class="osd-icon-btn focused" style="margin-top:22px;">✕ Cancelar (Atrás)</button>' +
      "</div>" +
      "<style>@keyframes spin { to { transform: rotate(360deg); } }</style>";

    document.body.appendChild(overlay);

    document.getElementById("load-cancel-btn")?.addEventListener("click", async function () {
      hideLoadingEvent();
      await invoke("cancel_episode_load").catch(() => { });
      updateFocus();
    });
  }

  const bgBlur = document.getElementById("load-bg-blur");
  const coverImg = document.getElementById("load-cover-img");
  if (bgBlur) bgBlur.style.backgroundImage = bannerUrl ? "url('" + bannerUrl + "')" : "none";
  if (coverImg) {
    if (coverUrl) {
      coverImg.src = coverUrl;
      coverImg.style.display = "block";
    } else {
      coverImg.style.display = "none";
    }
  }

  let elapsed = 0;
  loadDelayTimer = setTimeout(function () {
    document.getElementById("load-title").textContent = title + " — Episodio " + epNum;
    document.getElementById("load-status").textContent = "Buscando fuente 1080p y seleccionando subtítulos latinos...";
    overlay.style.display = "flex";
    document.getElementById("load-cancel-btn")?.focus();

    loadSecondsInterval = setInterval(function () {
      elapsed += 1;
      const statusEl = document.getElementById("load-status");
      if (!statusEl || statusEl.dataset.aiMode === "1") return;
      if (elapsed >= 5) {
        statusEl.textContent = "Cargando buffer de video en TorBox (" + elapsed + "s)...";
      } else {
        statusEl.textContent = "Conectando enlace directo (" + elapsed + "s)...";
      }
    }, 1000);
  }, 300);
}

function hideLoadingEvent() {
  currentPlayAttemptId++;
  isSearching = false;
  if (loadDelayTimer) {
    clearTimeout(loadDelayTimer);
    loadDelayTimer = null;
  }
  if (loadSecondsInterval) {
    clearInterval(loadSecondsInterval);
    loadSecondsInterval = null;
  }
  const statusEl = document.getElementById("load-status");
  if (statusEl) delete statusEl.dataset.aiMode;

  const wrap = document.getElementById("load-progress-wrap");
  const bar = document.getElementById("load-progress-bar");
  const pctEl = document.getElementById("load-progress-pct");
  if (wrap) wrap.style.display = "none";
  if (bar) bar.style.width = "0%";
  if (pctEl) pctEl.textContent = "0%";

  const aiToast = document.getElementById("ai-sub-toast");
  if (aiToast) {
    aiToast.classList.add("hidden");
    aiToast.style.display = "none";
  }

  const subLoader = document.getElementById("sub-zone-loader");
  if (subLoader) subLoader.style.display = "none";

  document.body.classList.remove("osd-transparent-mode");

  const overlay = document.getElementById("loading-overlay");
  if (overlay) overlay.style.display = "none";
}

/* =====================================================================
   CONTROL DEL REPRODUCTOR OSD (PC MPV + SINCRONIZACIÓN ANDROID TV)
   ===================================================================== */
function wakeOsd() {
  const osd = document.getElementById("player-osd");
  if (!osd || !isPlayerOpen || window.AndroidExo) return;

  osd.classList.remove("idle");
  if (osdHideTimer) clearTimeout(osdHideTimer);

  osdHideTimer = setTimeout(function () {
    const menu = document.getElementById("osd-settings-menu");
    const menuOpen = menu && !menu.classList.contains("hidden");
    if (!menuOpen && isPlayerOpen) {
      osd.classList.add("idle");
    }
  }, 3500);
}
let lastKnownTimePos = -1;
let stalledTicks = 0;

function setSeekBufferingOverlay(isBuffering) {
  let badge = document.getElementById("seek-buffer-badge");
  if (!badge) {
    badge = document.createElement("div");
    badge.id = "seek-buffer-badge";
    badge.style.cssText =
      "position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%);" +
      "background: rgba(14, 15, 18, 0.88); border: 1px solid #f47521; color: #ffffff;" +
      "padding: 16px 26px; border-radius: 10px; display: none; align-items: center; gap: 14px;" +
      "font-family: sans-serif; font-size: 15px; font-weight: 600; z-index: 1500;" +
      "box-shadow: 0 8px 25px rgba(0, 0, 0, 0.8); pointer-events: none;";
    badge.innerHTML =
      '<div style="width:24px; height:24px; border:3px solid #23252b; border-top-color:#f47521; border-radius:50%; animation: spin 0.8s linear infinite;"></div>' +
      "<span>Cargando escena desde TorBox...</span>";
    const osd = document.getElementById("player-osd");
    if (osd) osd.appendChild(badge);
  }
  badge.style.display = isBuffering ? "flex" : "none";
}

async function syncMpvState() {
  if (!isPlayerOpen || window.AndroidExo) return;
  try {
    const status = await invoke("mpv_status");
    currentTracks = status.tracks || [];
    currentDuration = status.duration || 0;

    if (!status.paused && status.duration > 0) {
      if (Math.abs(status.time_pos - lastKnownTimePos) < 0.05) {
        stalledTicks += 1;
        if (stalledTicks >= 2) setSeekBufferingOverlay(true);
      } else {
        stalledTicks = 0;
        setSeekBufferingOverlay(false);
      }
      lastKnownTimePos = status.time_pos;
    } else {
      stalledTicks = 0;
      setSeekBufferingOverlay(false);
    }

    const playBtn = document.getElementById("osd-play-btn");
    if (playBtn) playBtn.textContent = status.paused ? "▶" : "⏸";

    const timeDisplay = document.getElementById("osd-time-display");
    if (timeDisplay) {
      timeDisplay.textContent = formatTime(status.time_pos) + " / " + formatTime(status.duration);
    }

    if (status.duration > 0) {
      const pct = Math.min(100, Math.max(0, (status.time_pos / status.duration) * 100));
      const fill = document.getElementById("osd-progress-fill");
      const thumb = document.getElementById("osd-progress-thumb");
      if (fill) fill.style.width = pct + "%";
      if (thumb) thumb.style.left = pct + "%";
    }

    const activeAudio = currentTracks.find((t) => t.track_type === "audio" && t.selected);
    const activeSub = currentTracks.find((t) => t.track_type === "sub" && t.selected);

    const audioLabel = document.getElementById("current-audio-label");
    const subLabel = document.getElementById("current-sub-label");
    if (audioLabel && activeAudio) audioLabel.textContent = activeAudio.title + " ›";
    if (subLabel) subLabel.textContent = activeSub ? activeSub.title + " ›" : "Desactivado ›";

    if (status.duration > 60 && status.time_pos >= status.duration - 1.5 && !isSearching) {
      setSeekBufferingOverlay(false);
      if (activeAnime && activeEpisodeNum < totalSeasonEpisodes) {
        iniciarReproduccion(activeAnime, activeEpisodeNum + 1, totalSeasonEpisodes);
      } else {
        await closePlayerOverlay();
      }
    }
  } catch (_) { }
}

function openPlayerOverlay(title, epNum) {
  const epTitleEl = document.getElementById("osd-episode-title");
  if (epTitleEl) epTitleEl.textContent = title + " — Episodio " + epNum;

  // En Android TV el reproductor es nativo (PlayerActivity), no mostramos el OSD WebView
  if (window.AndroidExo) {
    isPlayerOpen = true;
    return;
  }

  isPlayerOpen = true;
  document.documentElement.classList.add("playing-mode");
  document.body.classList.add("playing-mode");

  const osd = document.getElementById("player-osd");
  if (osd) osd.classList.remove("hidden");

  wakeOsd();
  syncMpvState();

  if (osdPollInterval) clearInterval(osdPollInterval);
  osdPollInterval = setInterval(syncMpvState, 1000);
}

function resetAndroidPlayerStateOnReturn() {
  if (!window.AndroidExo) return;
  if (isPlayerOpen || document.body.classList.contains("playing-mode")) {
    isPlayerOpen = false;
    document.documentElement.classList.remove("playing-mode");
    document.body.classList.remove("playing-mode");
    document.getElementById("player-osd")?.classList.add("hidden");
    document.getElementById("osd-settings-menu")?.classList.add("hidden");
    updateFocus();
  }
}

// Exponer hook global para MainActivity.onResume() y enganchar al foco de ventana
window.onAndroidPlayerClosed = resetAndroidPlayerStateOnReturn;
window.addEventListener("focus", resetAndroidPlayerStateOnReturn);
document.addEventListener("visibilitychange", function () {
  if (!document.hidden) resetAndroidPlayerStateOnReturn();
});

async function closePlayerOverlay() {
  isPlayerOpen = false;
  if (osdPollInterval) {
    clearInterval(osdPollInterval);
    osdPollInterval = null;
  }
  if (osdHideTimer) {
    clearTimeout(osdHideTimer);
    osdHideTimer = null;
  }

  document.getElementById("player-osd")?.classList.add("hidden");
  document.getElementById("osd-settings-menu")?.classList.add("hidden");
  document.documentElement.classList.remove("playing-mode");
  document.body.classList.remove("playing-mode");

  const subLoader = document.getElementById("sub-zone-loader");
  if (subLoader) subLoader.style.display = "none";

  if (window.AndroidExo) {
    window.AndroidExo.stop();
  } else {
    await invoke("mpv_close_player").catch(() => { });
  }

  updateFocus();
}

function openTracksSubmenu(type) {
  document.getElementById("settings-main-view").classList.add("hidden");
  document.getElementById("settings-tracks-view").classList.remove("hidden");
  document.getElementById("submenu-title").textContent = type === "audio" ? "Audio" : "Subtitles/CC";

  const filtered = currentTracks.filter((t) => t.track_type === type);
  const container = document.getElementById("tracks-container");

  let html = "";
  if (type === "sub") {
    const anySelected = filtered.some((t) => t.selected);
    html +=
      '<div class="track-option ' + (!anySelected ? "selected" : "") + '" data-id="-1" data-type="sub" tabindex="0">' +
      '<div class="radio-circle"></div>' +
      "<span>Desactivado</span>" +
      "</div>";
  }

  for (let i = 0; i < filtered.length; i++) {
    const t = filtered[i];
    html +=
      '<div class="track-option ' + (t.selected ? "selected" : "") + '" data-id="' + t.id + '" data-type="' + type + '" tabindex="0">' +
      '<div class="radio-circle"></div>' +
      "<span>" + t.title + "</span>" +
      "</div>";
  }

  container.innerHTML = html;

  container.querySelectorAll(".track-option").forEach(function (el) {
    el.addEventListener("click", async function () {
      const id = parseInt(el.dataset.id, 10);
      if (window.AndroidExo) {
        window.AndroidExo.setTrack(type, id);
      } else {
        await invoke("mpv_set_track", { trackType: type, id: id === -1 ? 0 : id });
      }
      await syncMpvState();
      openTracksSubmenu(type);
    });
  });
}

/* =====================================================================
   NAVEGACIÓN ENTRE PESTAÑAS Y FICHA DE DETALLE
   ===================================================================== */
function switchTab(tabName, restoreCardIdx) {
  activeTab = tabName;

  if (tabName === "inicio") sideIndex = 0;
  else if (tabName === "catalogo") sideIndex = 1;
  else if (tabName === "buscar") sideIndex = 2;

  document.querySelectorAll(".nav-btn").forEach(function (btn) {
    btn.classList.toggle("active", btn.dataset.tab === tabName);
  });
  document.querySelectorAll(".tab-view").forEach(function (view) {
    view.classList.toggle("hidden", view.id !== "view-" + tabName);
  });

  if (tabName === "inicio") {
    ANIMES = TODAY_ANIMES.length > 0 ? TODAY_ANIMES.slice() : AIRING_CATALOG.slice(0, 15);
    cardIndex = typeof restoreCardIdx === "number" ? Math.min(restoreCardIdx, Math.max(0, ANIMES.length - 1)) : 0;
    currentArea = "cards";
    renderCards();
    updateFocus();
  } else if (tabName === "catalogo") {
    if (typeof restoreCardIdx === "number" && ANIMES.length > 0) {
      cardIndex = Math.min(restoreCardIdx, ANIMES.length - 1);
      currentArea = "cards";
      updateFocus();
    } else {
      currentArea = "cards";
      applyCatalogFilters(1);
    }
  } else if (tabName === "buscar") {
    const grid = document.getElementById("search-grid");
    const existingCards = grid ? grid.querySelectorAll(".anime-card").length : 0;
    if (typeof restoreCardIdx === "number" && existingCards > 0) {
      cardIndex = Math.min(restoreCardIdx, existingCards - 1);
      currentArea = "cards";
      updateFocus();
    } else {
      currentArea = "search_bar";
      searchBarIndex = 0;
      updateFocus();
    }
  }
}

function openAnimeDetail(anime) {
  if (!anime) return;
  if (activeTab !== "detail") {
    previousTabBeforeDetail = activeTab;
    savedCardIndexBeforeDetail = cardIndex;
  }
  activeTab = "detail";
  currentArea = "detail";
  detailSection = "actions";
  detailActionIndex = 0;
  detailRangeIndex = 0;
  detailGridIndex = 0;

  document.getElementById("episode-bar")?.classList.add("hidden");
  document.querySelectorAll(".tab-view").forEach(function (v) {
    v.classList.toggle("hidden", v.id !== "view-detail");
  });

  const match = (anime.ep || "").match(/Episodio\s+(\d+)/);
  const parsedEp = match && match[1] ? parseInt(match[1], 10) : 1;
  const maxEp = Math.max(anime.total_episodes || 1, parsedEp);

  document.getElementById("detail-hero").style.backgroundImage =
    "url('" + (anime.banner || anime.cover) + "')";
  document.getElementById("detail-cover").src = anime.cover;
  document.getElementById("detail-title").textContent = anime.title;
  document.getElementById("detail-desc").textContent = anime.desc;
  document.getElementById("detail-genres").textContent = (anime.genres && anime.genres.length > 0 ? anime.genres.slice(0, 3).join(" • ") : "ANIME") +
    " • SUB ESPAÑOL";
  document.getElementById("detail-ep-count").textContent =
    "Episodios disponibles (" + maxEp + ")";
  const playFirstBtn = document.getElementById("detail-play-first");
  const playLatestBtn = document.getElementById("detail-play-latest");
  playFirstBtn.textContent = "▶ Ver Episodio 1";
  playLatestBtn.textContent = "⚡ Ver Último (Ep. " + maxEp + ")";
  playFirstBtn.onclick = function () {
    iniciarReproduccion(anime, 1, maxEp);
  };
  playLatestBtn.onclick = function () {
    iniciarReproduccion(anime, maxEp, maxEp);
  };
  renderDetailEpisodes(anime, maxEp);
  document.getElementById("view-detail").scrollTop = 0;
  updateFocus();
}

function renderDetailEpisodes(anime, maxEp) {
  const pillsContainer = document.getElementById("detail-range-pills");
  const gridContainer = document.getElementById("detail-episodes-grid");
  pillsContainer.innerHTML = "";
  gridContainer.innerHTML = "";

  const drawRange = function (startEp, endEp) {
    let html = "";
    for (let ep = startEp; ep <= endEp; ep++) {
      html +=
        '<button class="detail-ep-card" data-ep="' + ep + '">' +
        "▶ Episodio " + ep +
        "</button>";
    }
    gridContainer.innerHTML = html;
    gridContainer.querySelectorAll(".detail-ep-card").forEach(function (btn, idx) {
      btn.addEventListener("click", function () {
        detailSection = "grid";
        detailGridIndex = idx;
        const epNum = parseInt(btn.dataset.ep, 10);
        iniciarReproduccion(anime, epNum, maxEp);
      });
    });
  };

  if (maxEp <= 25) {
    drawRange(1, maxEp);
    return;
  }

  const ranges = [];
  const lastStart = Math.max(1, maxEp - 9);
  ranges.push({ label: "Últimos (" + lastStart + " - " + maxEp + ")", start: lastStart, end: maxEp });

  const step = maxEp > 200 ? 100 : 50;
  for (let start = 1; start <= maxEp; start += step) {
    const end = Math.min(maxEp, start + step - 1);
    ranges.push({ label: start + " - " + end, start: start, end: end });
  }

  ranges.forEach(function (r, idx) {
    const pill = document.createElement("button");
    pill.className = "range-pill" + (idx === 0 ? " active" : "");
    pill.textContent = r.label;
    pill.addEventListener("click", function () {
      pillsContainer.querySelectorAll(".range-pill").forEach((p) => p.classList.remove("active"));
      pill.classList.add("active");
      detailRangeIndex = idx;
      detailGridIndex = 0;
      drawRange(r.start, r.end);
      updateFocus();
    });
    pillsContainer.appendChild(pill);
  });

  drawRange(ranges[0].start, ranges[0].end);
}

function buildCardsHtml(list, isCatalogMode) {
  let html = "";
  for (let idx = 0; idx < list.length; idx++) {
    const anime = list[idx];
    const subLabel = isCatalogMode
      ? ((anime.genres && anime.genres.length > 0 ? anime.genres.slice(0, 2).join(" • ") + " • " : "") + "Sub Español")
      : anime.ep;

    html +=
      '<div class="anime-card" data-index="' + idx + '">' +
      '<img src="' + anime.cover + '" alt="' + anime.title + '" />' +
      '<div class="card-meta">' +
      '<div class="card-title">' + anime.title + "</div>" +
      '<div class="card-ep">' + subLabel + "</div>" +
      "</div></div>";
  }
  return html;
}

function bindCardClicks(container, list) {
  container.querySelectorAll(".anime-card").forEach(function (card, idx) {
    card.addEventListener("click", function () {
      ANIMES = list;
      cardIndex = idx;
      currentArea = "cards";
      updateFocus();
      openAnimeDetail(list[idx]);
    });
  });
}

function renderCards() {
  const row = document.getElementById("cards-row");
  if (!row) return;
  row.innerHTML = buildCardsHtml(ANIMES, false);
  bindCardClicks(row, ANIMES);
}

let catalogPage = 1;
const CATALOG_PAGE_SIZE = 25;
const CATALOG_MAX_PAGES = 10;

function renderCatalogRangePills() {
  const pillsWrap = document.getElementById("catalog-range-pills");
  if (!pillsWrap) return;
  pillsWrap.innerHTML = "";

  for (let p = 1; p <= CATALOG_MAX_PAGES; p++) {
    const start = (p - 1) * CATALOG_PAGE_SIZE + 1;
    const end = p * CATALOG_PAGE_SIZE;
    const pill = document.createElement("button");
    pill.className = "range-pill" + (p === catalogPage ? " active" : "");
    pill.textContent = start + " - " + end;
    pill.addEventListener("click", function () {
      applyCatalogFilters(p);
    });
    pillsWrap.appendChild(pill);
  }
}

async function applyCatalogFilters(targetPage) {
  catalogPage = typeof targetPage === "number" ? targetPage : 1;

  const genreVal = document.getElementById("filter-genre")?.value || "ALL";
  const sortVal = document.getElementById("filter-sort")?.value || "POPULAR";
  const yearVal = document.getElementById("filter-year")?.value || "";
  const formatVal = document.getElementById("filter-format")?.value || "";
  const grid = document.getElementById("catalog-grid");

  renderCatalogRangePills();
  if (grid) {
    grid.innerHTML =
      '<p style="color:#aaa; padding:20px;">Cargando página ' +
      catalogPage +
      " (" +
      ((catalogPage - 1) * 25 + 1) +
      " - " +
      catalogPage * 25 +
      ")...</p>";
  }

  try {
    const pageItems = await invoke("get_catalog_page", {
      page: catalogPage,
      genre: genreVal,
      sort: sortVal,
      year: yearVal !== "" ? yearVal : null,
      format: formatVal !== "" ? formatVal : null,
    });
    ANIMES = pageItems || [];

    cardIndex = 0;
    if (grid) {
      grid.innerHTML =
        ANIMES.length > 0
          ? buildCardsHtml(ANIMES, true)
          : '<p style="color:#aaa; padding:20px;">No hay más animes en este rango o género.</p>';
      bindCardClicks(grid, ANIMES);
      grid.scrollTop = 0;
    }
    updateFocus();
  } catch (err) {
    if (grid) {
      grid.innerHTML = '<p style="color:#f47521; padding:20px;">Error cargando catálogo: ' + err + "</p>";
    }
  }
}

function renderEpisodeBar(anime, maxEp, currentEp) {
  const bar = document.getElementById("episode-bar");
  const list = document.getElementById("episode-list");
  const label = document.getElementById("now-playing-text");

  if (!bar || !list || !label) return;

  label.textContent = "Último visto: " + anime.title + " — Episodio " + currentEp + " (Sub Español)";

  const count = Math.max(maxEp, currentEp, 1);
  let html = "";
  for (let i = 1; i <= count; i++) {
    const isCurrent = i === currentEp ? "current" : "";
    html += '<button class="ep-pill ' + isCurrent + '" data-ep="' + i + '">Ep. ' + i + "</button>";
  }
  list.innerHTML = html;
  bar.classList.remove("hidden");

  list.querySelectorAll(".ep-pill").forEach(function (btn, idx) {
    btn.addEventListener("click", function () {
      episodeIndex = idx;
      iniciarReproduccion(anime, idx + 1, count);
    });
  });
}

function updateHero(anime) {
  if (!anime) return;
  document.getElementById("hero-title").textContent = anime.title;
  document.getElementById("hero-desc").textContent = anime.desc;
  document.getElementById("hero-banner").style.backgroundImage = "url('" + (anime.banner || anime.cover) + "')";
}

/* =====================================================================
   CÁLCULO DE COLUMNAS Y SISTEMA CENTRALIZADO DE FOCO D-PAD
   ===================================================================== */
function getColumnsCountInContainer(containerSelector, itemSelector) {
  const container = document.querySelector(containerSelector);
  if (!container) return 1;
  const items = container.querySelectorAll(itemSelector);
  if (items.length < 2) return 1;
  const firstTop = items[0].offsetTop;
  let cols = 0;
  for (let i = 0; i < items.length; i++) {
    if (items[i].offsetTop === firstTop) cols++;
    else break;
  }
  return Math.max(1, cols);
}

function isEpisodeBarVisible() {
  const bar = document.getElementById("episode-bar");
  return bar && !bar.classList.contains("hidden");
}

function updateFocus() {
  document.querySelectorAll(".focused").forEach(function (el) {
    el.classList.remove("focused");
  });

  if (activeTab === "detail") {
    if (detailSection === "back") {
      const btn = document.getElementById("detail-back-btn");
      btn?.classList.add("focused");
      btn?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    } else if (detailSection === "actions") {
      const btns = [
        document.getElementById("detail-play-first"),
        document.getElementById("detail-play-latest"),
      ].filter(Boolean);
      detailActionIndex = Math.max(0, Math.min(detailActionIndex, btns.length - 1));
      btns[detailActionIndex]?.classList.add("focused");
      btns[detailActionIndex]?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    } else if (detailSection === "ranges") {
      const pills = document.querySelectorAll("#detail-range-pills .range-pill");
      if (pills.length > 0) {
        detailRangeIndex = Math.max(0, Math.min(detailRangeIndex, pills.length - 1));
        pills[detailRangeIndex].classList.add("focused");
        pills[detailRangeIndex].scrollIntoView({ behavior: "smooth", inline: "center", block: "nearest" });
      }
    } else if (detailSection === "grid") {
      const epCards = document.querySelectorAll("#detail-episodes-grid .detail-ep-card");
      if (epCards.length > 0) {
        detailGridIndex = Math.max(0, Math.min(detailGridIndex, epCards.length - 1));
        epCards[detailGridIndex].classList.add("focused");
        epCards[detailGridIndex].scrollIntoView({ behavior: "smooth", block: "nearest" });
      }
    }
    return;
  }

  if (currentArea === "cards" && ANIMES.length > 0) {
    const activeContainer = document.getElementById("view-" + activeTab) || document;
    const cards = activeContainer.querySelectorAll(".anime-card");
    cardIndex = Math.max(0, Math.min(cardIndex, cards.length - 1));
    if (cards[cardIndex]) {
      cards[cardIndex].classList.add("focused");
      cards[cardIndex].scrollIntoView({ behavior: "smooth", inline: "center", block: "nearest" });
      if (activeTab === "inicio") {
        updateHero(ANIMES[cardIndex]);
      }
    }
  } else if (currentArea === "filters" && activeTab === "catalogo") {
    const filterItems = Array.from(
      document.querySelectorAll("#view-catalogo .cr-select, #catalog-range-pills .range-pill")
    );
    filterIndex = Math.max(0, Math.min(filterIndex, filterItems.length - 1));
    if (filterItems[filterIndex]) {
      filterItems[filterIndex].classList.add("focused");
      filterItems[filterIndex].scrollIntoView({ behavior: "smooth", inline: "center", block: "nearest" });
    }
  } else if (currentArea === "search_bar" && activeTab === "buscar") {
    const input = document.getElementById("anime-search-input");
    const btn = document.getElementById("anime-search-btn");
    if (searchBarIndex === 0 && input) {
      input.classList.add("focused");
    } else if (btn) {
      btn.classList.add("focused");
    }
  } else if (currentArea === "play") {
    document.getElementById("hero-play")?.classList.add("focused");
  } else if (currentArea === "sidebar") {
    const navBtns = document.querySelectorAll(".nav-btn");
    sideIndex = Math.max(0, Math.min(sideIndex, navBtns.length - 1));
    if (navBtns[sideIndex]) {
      navBtns[sideIndex].classList.add("focused");
    }
  } else if (currentArea === "episodes") {
    const pills = document.querySelectorAll("#episode-list .ep-pill");
    episodeIndex = Math.max(0, Math.min(episodeIndex, pills.length - 1));
    if (pills[episodeIndex]) {
      pills[episodeIndex].classList.add("focused");
      pills[episodeIndex].scrollIntoView({ behavior: "smooth", inline: "center", block: "nearest" });
    }
  }
}
/* =====================================================================
   EJECUCIÓN DE REPRODUCCIÓN
   ===================================================================== */
async function iniciarReproduccion(anime, customEp, maxKnownEp) {
  if (!anime) return;
  if (isSearching) {
    currentPlayAttemptId++;
    isSearching = false;
    hideLoadingEvent();
    invoke("cancel_episode_load").catch(() => { });
  }

  const attemptId = ++currentPlayAttemptId;
  isSearching = true;

  const aiToast = document.getElementById("ai-sub-toast");
  if (aiToast) aiToast.classList.add("hidden");
  document.body.classList.remove("osd-transparent-mode");

  const match = (anime.ep || "").match(/Episodio\s+(\d+)/);
  const matchedEpStr = match ? match.at(1) : null;
  const latestEp = maxKnownEp || (matchedEpStr ? parseInt(matchedEpStr, 10) : 12);
  const epToPlay = customEp || latestEp;
  const playBtn = document.getElementById("hero-play");
  const statusText = document.getElementById("now-playing-text");
  const textoOriginal = playBtn ? playBtn.textContent : "▶ Ver Detalles y Episodios";

  if (playBtn && activeTab === "inicio") {
    playBtn.textContent = "⏳ Cargando Ep. " + epToPlay + "...";
  }
  if (statusText) {
    statusText.textContent = "⏳ Preparando: " + anime.title + " — Episodio " + epToPlay + "...";
  }

  showLoadingEvent(anime, epToPlay);

  try {
    await invoke("play_anime_episode", {
      title: anime.title,
      titleRomaji: anime.title_romaji,
      synonyms: anime.synonyms || [],
      episode: epToPlay,
      anilistId: anime.id || 0,
    });

    if (attemptId !== currentPlayAttemptId) return;

    activeAnime = anime;
    activeEpisodeNum = epToPlay;
    totalSeasonEpisodes = latestEp;
    episodeIndex = epToPlay - 1;

    if (activeTab !== "detail") {
      renderEpisodeBar(anime, totalSeasonEpisodes, activeEpisodeNum);
      currentArea = "episodes";
    } else {
      detailSection = "grid";
      const epCards = Array.from(document.querySelectorAll("#detail-episodes-grid .detail-ep-card"));
      const foundIdx = epCards.findIndex((c) => parseInt(c.dataset.ep, 10) === epToPlay);
      if (foundIdx >= 0) detailGridIndex = foundIdx;
    }

    openPlayerOverlay(anime.title, epToPlay);
  } catch (err) {
    if (attemptId !== currentPlayAttemptId) return;

    if (aiToast) aiToast.classList.add("hidden");
    hideLoadingEvent();
    document.body.classList.remove("osd-transparent-mode");

    const msg = String(err || "");
    if (!msg.toLowerCase().includes("cancelada")) {
      showTvAlert("Error al reproducir", msg);
    }
  } finally {
    if (attemptId === currentPlayAttemptId) {
      hideLoadingEvent();
      if (playBtn && activeTab === "inicio") playBtn.textContent = textoOriginal;
      isSearching = false;
    }
  }
}

/* =====================================================================
   EVENTOS DE TECLADO, RATÓN Y CONTROL REMOTO (D-PAD / GOOGLE TV)
   ===================================================================== */
window.addEventListener("mousemove", wakeOsd);
window.addEventListener("keydown", async function (e) {
  const alertModal = document.getElementById("tv-alert-modal");
  if (alertModal && !alertModal.classList.contains("hidden")) {
    if (e.key === "Enter" || e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      hideTvAlert();
    }
    return;
  }

  if (isSearching) {
    if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      document.getElementById("load-cancel-btn")?.click();
    }
    return;
  }

  if (document.activeElement && document.activeElement.tagName === "INPUT") {
    return;
  }

  if (isPlayerOpen && !window.AndroidExo) {
    wakeOsd();
    const menu = document.getElementById("osd-settings-menu");
    if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      if (menu && !menu.classList.contains("hidden")) {
        menu.classList.add("hidden");
      } else {
        await closePlayerOverlay();
      }
    } else if (e.key === " " || e.key === "Enter" || e.key === "MediaPlayPause") {
      e.preventDefault();
      await invoke("mpv_toggle_pause");
      await syncMpvState();
    } else if (e.key === "ArrowRight" || e.key === "MediaFastForward") {
      e.preventDefault();
      await invoke("mpv_seek_relative", { offset: 10 });
      await syncMpvState();
    } else if (e.key === "ArrowLeft" || e.key === "MediaRewind") {
      e.preventDefault();
      await invoke("mpv_seek_relative", { offset: -10 });
      await syncMpvState();
    }
    return;
  }

  if (activeTab === "detail") {
    if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      switchTab(previousTabBeforeDetail, savedCardIndexBeforeDetail);
      return;
    }

    const ranges = document.querySelectorAll("#detail-range-pills .range-pill");
    const epCards = document.querySelectorAll("#detail-episodes-grid .detail-ep-card");
    const gridCols = getColumnsCountInContainer("#detail-episodes-grid", ".detail-ep-card");

    if (detailSection === "back") {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        detailSection = "actions";
        detailActionIndex = 0;
      } else if (e.key === "Enter") {
        e.preventDefault();
        switchTab(previousTabBeforeDetail, savedCardIndexBeforeDetail);
        return;
      }
    } else if (detailSection === "actions") {
      if (e.key === "ArrowUp") {
        e.preventDefault();
        detailSection = "back";
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        if (detailActionIndex < 1) detailActionIndex++;
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        if (detailActionIndex > 0) detailActionIndex--;
        else detailSection = "back";
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        if (ranges.length > 0) {
          detailSection = "ranges";
        } else if (epCards.length > 0) {
          detailSection = "grid";
          detailGridIndex = 0;
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        const btn = detailActionIndex === 0
          ? document.getElementById("detail-play-first")
          : document.getElementById("detail-play-latest");
        btn?.click();
        return;
      }
    } else if (detailSection === "ranges") {
      if (e.key === "ArrowUp") {
        e.preventDefault();
        detailSection = "actions";
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        if (detailRangeIndex < ranges.length - 1) detailRangeIndex++;
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        if (detailRangeIndex > 0) detailRangeIndex--;
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        if (epCards.length > 0) {
          detailSection = "grid";
          detailGridIndex = 0;
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        ranges.item(detailRangeIndex)?.click();
        return;
      }
    } else if (detailSection === "grid") {
      if (e.key === "ArrowRight") {
        e.preventDefault();
        if (detailGridIndex < epCards.length - 1) detailGridIndex++;
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        if (detailGridIndex > 0) detailGridIndex--;
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        if (detailGridIndex + gridCols < epCards.length) {
          detailGridIndex += gridCols;
        }
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        if (detailGridIndex - gridCols >= 0) {
          detailGridIndex -= gridCols;
        } else if (ranges.length > 0) {
          detailSection = "ranges";
        } else {
          detailSection = "actions";
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        epCards.item(detailGridIndex)?.click();
        return;
      }
    }

    updateFocus();
    return;
  }

  if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
    e.preventDefault();
    if (currentArea === "episodes") {
      currentArea = "cards";
    } else if (activeTab !== "inicio") {
      switchTab("inicio");
      return;
    } else if (currentArea !== "sidebar") {
      currentArea = "sidebar";
    }
    updateFocus();
    return;
  }

  const navBtns = document.querySelectorAll(".nav-btn");

  if (currentArea === "cards") {
    const colsPerRow = activeTab === "inicio"
      ? 1
      : getColumnsCountInContainer("#view-" + activeTab + " .catalog-grid", ".anime-card");
    const isFirstCol = activeTab === "inicio" ? cardIndex === 0 : cardIndex % colsPerRow === 0;

    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (cardIndex < ANIMES.length - 1) cardIndex++;
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (isFirstCol) {
        currentArea = "sidebar";
      } else if (cardIndex > 0) {
        cardIndex--;
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (activeTab === "inicio") {
        if (isEpisodeBarVisible()) currentArea = "episodes";
      } else if (cardIndex + colsPerRow < ANIMES.length) {
        cardIndex += colsPerRow;
      } else if (activeTab === "catalogo" && ANIMES.length === CATALOG_PAGE_SIZE && catalogPage < CATALOG_MAX_PAGES) {
        applyCatalogFilters(catalogPage + 1);
        return;
      } else if (isEpisodeBarVisible()) {
        currentArea = "episodes";
      }
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (activeTab === "inicio") {
        currentArea = "play";
      } else if (cardIndex - colsPerRow >= 0) {
        cardIndex -= colsPerRow;
      } else if (activeTab === "catalogo") {
        currentArea = "filters";
        filterIndex = 0;
      } else if (activeTab === "buscar") {
        currentArea = "search_bar";
        searchBarIndex = 0;
      }
    } else if (e.key === "Enter" && ANIMES.at(cardIndex)) {
      e.preventDefault();
      openAnimeDetail(ANIMES.at(cardIndex));
      return;
    }
  } else if (currentArea === "filters") {
    const filterItems = Array.from(
      document.querySelectorAll("#view-catalogo .cr-select, #catalog-range-pills .range-pill")
    );
    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (filterIndex < filterItems.length - 1) filterIndex++;
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (filterIndex > 0) filterIndex--;
      else currentArea = "sidebar";
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (ANIMES.length > 0) currentArea = "cards";
    } else if (e.key === "Enter" && filterItems.at(filterIndex)) {
      e.preventDefault();
      const targetEl = filterItems.at(filterIndex);
      if (targetEl.tagName === "SELECT") {
        targetEl.selectedIndex = (targetEl.selectedIndex + 1) % targetEl.options.length;
        targetEl.dispatchEvent(new Event("change"));
      } else {
        targetEl.click();
      }
      return;
    }
  } else if (currentArea === "search_bar") {
    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (searchBarIndex === 0) searchBarIndex = 1;
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (searchBarIndex === 1) searchBarIndex = 0;
      else currentArea = "sidebar";
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (ANIMES.length > 0) currentArea = "cards";
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (searchBarIndex === 0) {
        document.getElementById("anime-search-input")?.focus();
      } else {
        document.getElementById("anime-search-btn")?.click();
      }
      return;
    }
  } else if (currentArea === "play") {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      currentArea = "cards";
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      currentArea = "sidebar";
    } else if (e.key === "Enter" && ANIMES.at(cardIndex)) {
      e.preventDefault();
      openAnimeDetail(ANIMES.at(cardIndex));
      return;
    }
  } else if (currentArea === "episodes") {
    const pills = document.querySelectorAll("#episode-list .ep-pill");
    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (episodeIndex < pills.length - 1) episodeIndex++;
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (episodeIndex > 0) episodeIndex--;
      else currentArea = "sidebar";
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      currentArea = "cards";
    } else if (e.key === "Enter" && pills.item(episodeIndex)) {
      e.preventDefault();
      pills.item(episodeIndex).click();
      return;
    }
  } else if (currentArea === "sidebar") {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (sideIndex < navBtns.length - 1) sideIndex++;
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (sideIndex > 0) sideIndex--;
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      if (activeTab === "buscar" && ANIMES.length === 0) {
        currentArea = "search_bar";
        searchBarIndex = 0;
      } else if (ANIMES.length > 0) {
        currentArea = "cards";
      } else if (activeTab === "catalogo") {
        currentArea = "filters";
      }
    } else if (e.key === "Enter" && navBtns.item(sideIndex)) {
      e.preventDefault();
      navBtns.item(sideIndex).click();
      return;
    }
  }
  updateFocus();
});
/* =====================================================================
INICIALIZACIÓN DEL DOM Y LISTENERS DE TAURI
===================================================================== */
window.addEventListener("DOMContentLoaded", async function () {
  document.getElementById("tv-alert-ok-btn")?.addEventListener("click", hideTvAlert);
  const heroPlay = document.getElementById("hero-play");
  if (heroPlay) {
    heroPlay.addEventListener("click", function () {
      if (ANIMES.at(cardIndex)) openAnimeDetail(ANIMES.at(cardIndex));
    });
  }
  const backBtn = document.getElementById("osd-back-btn");
  if (backBtn) backBtn.addEventListener("click", closePlayerOverlay);
  const playBtn = document.getElementById("osd-play-btn");
  if (playBtn) {
    playBtn.addEventListener("click", async function () {
      if (window.AndroidExo) window.AndroidExo.pauseToggle();
      else await invoke("mpv_toggle_pause");
      await syncMpvState();
    });
  }
  const nextBtn = document.getElementById("osd-next-btn");
  if (nextBtn) {
    nextBtn.addEventListener("click", function () {
      if (activeAnime) {
        iniciarReproduccion(activeAnime, activeEpisodeNum + 1, totalSeasonEpisodes);
      }
    });
  }
  const gearBtn = document.getElementById("osd-gear-btn");
  if (gearBtn) {
    gearBtn.addEventListener("click", function () {
      const menu = document.getElementById("osd-settings-menu");
      menu.classList.toggle("hidden");
      document.getElementById("settings-main-view").classList.remove("hidden");
      document.getElementById("settings-tracks-view").classList.add("hidden");
    });
  }
  document.getElementById("menu-open-audio")?.addEventListener("click", () => openTracksSubmenu("audio"));
  document.getElementById("menu-open-subs")?.addEventListener("click", () => openTracksSubmenu("sub"));
  document.getElementById("menu-back-btn")?.addEventListener("click", function () {
    document.getElementById("settings-tracks-view").classList.add("hidden");
    document.getElementById("settings-main-view").classList.remove("hidden");
  });
  const progressBar = document.getElementById("osd-progress-bar");
  if (progressBar) {
    progressBar.addEventListener("click", async function (e) {
      if (!currentDuration) return;
      const rect = e.currentTarget.getBoundingClientRect();
      const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
      if (window.AndroidExo) window.AndroidExo.seekTo(ratio * currentDuration);
      else await invoke("mpv_seek", { seconds: ratio * currentDuration });
      await syncMpvState();
    });
  }
  document.getElementById("hero-title").textContent = "Cargando cartelera...";
  TODAY_ANIMES = [];
  try {
    const [todayRes, allRes] = await Promise.all([
      invoke("get_today_anime").catch(() => []),
      invoke("get_airing_anime"),
    ]);
    AIRING_CATALOG = allRes || [];
    TODAY_ANIMES = todayRes && todayRes.length > 0 ? todayRes : AIRING_CATALOG.slice(0, 15);
    ANIMES = TODAY_ANIMES.slice();
    renderCards();
    updateFocus();
  } catch (err) {
    document.getElementById("hero-title").textContent = "Error cargando datos";
    document.getElementById("hero-desc").textContent = String(err);
  }
  document.querySelectorAll(".nav-btn").forEach(function (btn, idx) {
    btn.addEventListener("click", function () {
      sideIndex = idx;
      switchTab(btn.dataset.tab);
    });
  });
  document.getElementById("filter-genre")?.addEventListener("change", () => applyCatalogFilters(1));
  document.getElementById("filter-sort")?.addEventListener("change", () => applyCatalogFilters(1));
  document.getElementById("filter-year")?.addEventListener("change", () => applyCatalogFilters(1));
  document.getElementById("filter-format")?.addEventListener("change", () => applyCatalogFilters(1));


  const runSearch = async function () {
    const input = document.getElementById("anime-search-input");
    const grid = document.getElementById("search-grid");
    const q = input ? input.value.trim() : "";
    if (!q || !grid) return;
    grid.innerHTML = 'Buscando "' + q + '"...';
    try {
      const results = await invoke("search_anime", { query: q });
      ANIMES = results || [];
      cardIndex = 0;
      grid.innerHTML = ANIMES.length > 0
        ? buildCardsHtml(ANIMES, true)
        : 'No se encontraron resultados para "' + q + '".';
      bindCardClicks(grid, ANIMES);
      if (ANIMES.length > 0) {
        currentArea = "cards";
        updateFocus();
      }
    } catch (err) {
      grid.innerHTML = 'Error: ' + err + "";
    }
  };
  document.getElementById("anime-search-btn")?.addEventListener("click", runSearch);
  document.getElementById("anime-search-input")?.addEventListener("keydown", async function (e) {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      this.blur();
      await runSearch();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      e.stopPropagation();
      this.blur();
      if (ANIMES.length > 0) {
        cardIndex = 0;
        currentArea = "cards";
      } else {
        searchBarIndex = 1;
        currentArea = "search_bar";
      }
      updateFocus();
    } else if (e.key === "Escape" || e.key === "GoBack") {
      e.preventDefault();
      e.stopPropagation();
      this.blur();
      currentArea = ANIMES.length > 0 ? "cards" : "search_bar";
      updateFocus();
    } else {
      e.stopPropagation();
    }
  });
  document.getElementById("close-episode-bar-btn")?.addEventListener("click", function () {
    document.getElementById("episode-bar")?.classList.add("hidden");
    currentArea = "cards";
    updateFocus();
  });
  document.getElementById("detail-back-btn")?.addEventListener("click", function () {
    switchTab(previousTabBeforeDetail, savedCardIndexBeforeDetail);
  });

  const tauriEvent = window.__TAURI__ && window.__TAURI__.event;
  if (tauriEvent) {
    tauriEvent.listen("android-exo-play", function (event) {
      const data = event.payload || {};
      console.log("[AndroidExo] Evento recibido:", JSON.stringify(data));
      if (window.AndroidExo && data.url) {
        const titleEl = document.getElementById("osd-episode-title");
        const epTitle = titleEl && titleEl.textContent ? titleEl.textContent.trim() : "";
        if (window.AndroidExo.playWithTitle) {
          window.AndroidExo.playWithTitle(data.url, data.subtitlePath || "", epTitle);
        } else {
          window.AndroidExo.play(data.url, data.subtitlePath || "");
        }
      }
    });

    tauriEvent.listen("ai-sub-status", function (event) {
      const state = event.payload;
      let subLoader = document.getElementById("sub-zone-loader");
      if (!subLoader) {
        subLoader = document.createElement("div");
        subLoader.id = "sub-zone-loader";
        subLoader.style.cssText =
          "position: fixed; bottom: 115px; left: 50%; transform: translateX(-50%);" +
          "background: rgba(11, 11, 13, 0.88); border: 1px solid rgba(244, 117, 33, 0.7);" +
          "color: #ffffff; padding: 10px 24px; border-radius: 6px; display: none;" +
          "align-items: center; gap: 12px; font-family: sans-serif; font-size: 16px;" +
          "font-weight: 600; z-index: 1400; box-shadow: 0 4px 18px rgba(0, 0, 0, 0.85);" +
          "pointer-events: none; transition: opacity 0.25s ease;";
        document.body.appendChild(subLoader);
      }
      if (state === "loading") {
        if (!isSearching) return;
        const ov = document.getElementById("loading-overlay");
        if (ov) ov.style.display = "flex";
        const loadStatus = document.getElementById("load-status");
        if (loadStatus) {
          loadStatus.dataset.aiMode = "1";
          loadStatus.textContent = "Creando subtítulo en Español Latino...";
        }
        const wrap = document.getElementById("load-progress-wrap");
        const bar = document.getElementById("load-progress-bar");
        const pctEl = document.getElementById("load-progress-pct");
        const stepEl = document.getElementById("load-progress-step");
        if (wrap) wrap.style.display = "block";
        if (bar && parseInt(bar.style.width || "0", 10) < 65) bar.style.width = "65%";
        if (pctEl && parseInt(pctEl.textContent || "0", 10) < 65) pctEl.textContent = "65%";
        if (stepEl) stepEl.textContent = "Traduciendo subtítulos con IA...";
        subLoader.innerHTML = "Creando subtítulo en Español Latino...";
        subLoader.style.display = "flex";
        subLoader.style.opacity = "1";
      } else if (state === "done") {
        subLoader.innerHTML = "✓ Subtítulos en esp latino listos";
        setTimeout(function () {
          subLoader.style.opacity = "0";
          setTimeout(function () {
            subLoader.style.display = "none";
          }, 300);
        }, 2500);
      } else {
        subLoader.style.display = "none";
      }
    });

    tauriEvent.listen("load-progress", function (event) {
      if (!isSearching) return;
      const data = event.payload || {};
      const ov = document.getElementById("loading-overlay");
      const wrap = document.getElementById("load-progress-wrap");
      const bar = document.getElementById("load-progress-bar");
      const pctEl = document.getElementById("load-progress-pct");
      const stepEl = document.getElementById("load-progress-step");
      const statusEl = document.getElementById("load-status");
      if (ov) ov.style.display = "flex";
      if (wrap) wrap.style.display = "block";
      if (bar && typeof data.percent === "number") bar.style.width = data.percent + "%";
      if (pctEl && typeof data.percent === "number") pctEl.textContent = data.percent + "%";
      if (stepEl && data.label) stepEl.textContent = data.label;
      if (statusEl && data.label) {
        statusEl.dataset.aiMode = "1";
        statusEl.textContent = data.label;
      }
    });
  }
});
