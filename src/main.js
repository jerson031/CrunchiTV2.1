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
let currentPlayingFullTitle = "";
let activeTvSelectEl = null;
let tvSelectOptionIndex = 0;

function openTvSelectDropdown(selectEl) {
  if (!selectEl || selectEl.tagName !== "SELECT") return;
  activeTvSelectEl = selectEl;
  tvSelectOptionIndex = Math.max(0, selectEl.selectedIndex);

  let modal = document.getElementById("cr-tv-dropdown");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "cr-tv-dropdown";
    modal.className = "cr-tv-dropdown hidden";
    modal.innerHTML =
      '<div class="cr-tv-dropdown-panel">' +
      '<div id="cr-tv-dropdown-header" class="cr-tv-dropdown-header">Seleccionar opción</div>' +
      '<div id="cr-tv-dropdown-list" class="cr-tv-dropdown-list"></div>' +
      "</div>";
    document.body.appendChild(modal);
  }

  const header = document.getElementById("cr-tv-dropdown-header");
  const list = document.getElementById("cr-tv-dropdown-list");
  const firstOpt = selectEl.options && selectEl.options[0];
  const firstText = firstOpt && firstOpt.textContent ? firstOpt.textContent : "Filtro";
  header.textContent = firstText.includes(":") ? firstText.split(":")[0] : "Seleccionar opción";


  list.innerHTML = "";
  Array.from(selectEl.options).forEach(function (opt, idx) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "cr-tv-dropdown-item" + (idx === selectEl.selectedIndex ? " active-opt" : "");
    btn.textContent = opt.textContent;
    btn.addEventListener("click", function () {
      selectEl.selectedIndex = idx;
      selectEl.dispatchEvent(new Event("change"));
      closeTvSelectDropdown();
    });
    list.appendChild(btn);
  });

  modal.classList.remove("hidden");
  updateTvDropdownFocus();
}

function updateTvDropdownFocus() {
  const items = document.querySelectorAll("#cr-tv-dropdown-list .cr-tv-dropdown-item");
  if (items.length === 0) return;
  tvSelectOptionIndex = Math.max(0, Math.min(tvSelectOptionIndex, items.length - 1));
  items.forEach((el, i) => el.classList.toggle("focused", i === tvSelectOptionIndex));
  items[tvSelectOptionIndex].scrollIntoView({ behavior: "smooth", block: "nearest" });
}

function closeTvSelectDropdown() {
  const modal = document.getElementById("cr-tv-dropdown");
  if (modal) modal.classList.add("hidden");
  activeTvSelectEl = null;
  updateFocus();
}


// --- NUEVO BLOQUE: MODAL SELECTOR DE FUENTES ---
let activeSourceModalContext = null; // { anime, epNum, maxKnownEp, candidates }
let sourceModalIndex = 0;

function formatSourceBadges(cand) {
  const resLabel = cand.resolution_score === 3 ? "1080p" : cand.resolution_score === 2 ? "720p" : "SD/480p";
  const langLabel = cand.spanish_score >= 90 ? "🇪🇸 Sub Español" : cand.spanish_score >= 60 ? "🌐 Multi-Sub" : "🇬🇧 Inglés (IA)";
  const sizeLabel = cand.size_mb > 0 && cand.size_mb < 9000 ? cand.size_mb + " MB" : "Pack";
  let html = "";
  if (cand.is_cached) html += '<span class="source-tag cached">⚡ Caché TorBox</span>';
  html += '<span class="source-tag">' + cand.source + "</span>";
  html += '<span class="source-tag">' + resLabel + "</span>";
  html += '<span class="source-tag es">' + langLabel + "</span>";
  html += '<span class="source-tag">' + sizeLabel + " • " + cand.seeders + " seeds</span>";
  return html;
}

function openSourceSelectorModal(anime, epNum, maxKnownEp, candidates, reasonText) {
  hideLoadingEvent();
  activeSourceModalContext = { anime, epNum, maxKnownEp, candidates: candidates || [] };
  sourceModalIndex = 0;

  let modal = document.getElementById("cr-source-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "cr-source-modal";
    modal.className = "tv-modal hidden";
    modal.innerHTML =
      '<div class="source-modal-box">' +
      '<div class="source-modal-header">' +
      '<div>' +
      '<h3 id="source-modal-title">Seleccionar Fuente</h3>' +
      '<div id="source-modal-sub" class="source-modal-sub">Elige un release (se guardará para próximas veces)</div>' +
      "</div>" +
      '<button id="source-modal-close-btn" class="osd-icon-btn">✕ Cerrar (Atrás)</button>' +
      "</div>" +
      '<div id="source-modal-list" class="source-modal-list"></div>' +
      "</div>";
    document.body.appendChild(modal);

    document.getElementById("source-modal-close-btn")?.addEventListener("click", closeSourceSelectorModal);
  }

  document.getElementById("source-modal-title").textContent = anime.title + " — Episodio " + epNum;
  document.getElementById("source-modal-sub").textContent =
    reasonText || "Ordenado por porcentaje de coincidencia (▲ ▼ mover • OK elegir)";

  const listEl = document.getElementById("source-modal-list");
  listEl.innerHTML = "";

  if (!candidates || candidates.length === 0) {
    listEl.innerHTML = '<p style="color:#aaa; padding:20px; text-align:center;">No se encontraron fuentes disponibles para este episodio.</p>';
  } else {
    candidates.forEach(function (cand, idx) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "source-item-card";
      const pctClass = cand.total_pct >= 80 ? "high" : "";
      btn.innerHTML =
        '<div class="source-score-pill ' + pctClass + '">' + cand.total_pct + "%</div>" +
        '<div class="source-item-info">' +
        '<div class="source-item-title">' + cand.title + "</div>" +
        '<div class="source-item-tags">' + formatSourceBadges(cand) + "</div>" +
        "</div>";

      btn.addEventListener("click", function () {
        confirmManualSourceSelection(cand);
      });
      listEl.appendChild(btn);
    });
  }

  modal.classList.remove("hidden");
  updateSourceModalFocus();
}

function updateSourceModalFocus() {
  const items = document.querySelectorAll("#source-modal-list .source-item-card");
  if (items.length === 0) {
    document.getElementById("source-modal-close-btn")?.classList.add("focused");
    return;
  }
  sourceModalIndex = Math.max(0, Math.min(sourceModalIndex, items.length - 1));
  items.forEach((el, i) => el.classList.toggle("focused", i === sourceModalIndex));
  items[sourceModalIndex].scrollIntoView({ behavior: "smooth", block: "nearest" });
}

function closeSourceSelectorModal() {
  const modal = document.getElementById("cr-source-modal");
  if (modal) modal.classList.add("hidden");
  activeSourceModalContext = null;
  updateFocus();
}

// ── SELECTOR DE SUBTÍTULOS (mismo mecanismo que "Cambiar fuente") ───────────
let activeSubModalContext = null; // { anime, epNum, startMs, options }
let subModalIndex = 0;

function escHtml(v) {
  return String(v == null ? "" : v)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

window.openSubtitlePickerForCurrentEpisode = function (startMs) {
  const np = readNowPlaying();
  let anime = np ? np.anime : null;
  let ep = np ? np.ep : 1;
  if (!anime && typeof activeAnime !== "undefined" && activeAnime) {
    anime = activeAnime;
    ep = activeEpisodeNum;
  }
  if (!anime) return;
  openSubtitlePicker(anime, ep, startMs || 0);
};

async function openSubtitlePicker(anime, epNum, startMs) {
  if (!anime || isSearching) return;
  const attemptId = ++currentPlayAttemptId;
  isSearching = true;
  showLoadingEvent(anime, epNum);
  const statusEl = document.getElementById("load-status");
  if (statusEl) statusEl.textContent = "Buscando subtítulos en todos los proveedores...";
  try {
    const options = await invoke("search_subtitle_options", {
      anilistId: anime.id || 0,
      title: anime.title,
      titleRomaji: anime.title_romaji || "",
      synonyms: anime.synonyms || [],
      episode: epNum,
    });
    if (attemptId !== currentPlayAttemptId) return;
    openSubtitleModal(anime, epNum, startMs, options || []);
  } catch (err) {
    if (attemptId !== currentPlayAttemptId) return;
    hideLoadingEvent();
    showTvAlert("Sin subtítulos", String(err || "No se encontraron subtítulos."));
  } finally {
    if (attemptId === currentPlayAttemptId) isSearching = false;
  }
}

function openSubtitleModal(anime, epNum, startMs, options) {
  hideLoadingEvent();
  activeSubModalContext = { anime, epNum, startMs, options };
  subModalIndex = 0;

  let modal = document.getElementById("cr-sub-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "cr-sub-modal";
    modal.className = "tv-modal hidden";
    modal.innerHTML =
      '<div class="source-modal-box">' +
      '<div class="source-modal-header">' +
      "<div>" +
      '<h3 id="sub-modal-title">Seleccionar Subtítulo</h3>' +
      '<div id="sub-modal-sub" class="source-modal-sub"></div>' +
      "</div>" +
      '<button id="sub-modal-close-btn" class="osd-icon-btn">✕ Cerrar (Atrás)</button>' +
      "</div>" +
      '<div id="sub-modal-list" class="source-modal-list"></div>' +
      "</div>";
    document.body.appendChild(modal);
    document.getElementById("sub-modal-close-btn")?.addEventListener("click", closeSubtitleModal);
  }

  document.getElementById("sub-modal-title").textContent = anime.title + " — Episodio " + epNum;
  document.getElementById("sub-modal-sub").textContent =
    "Ordenados por coincidencia (▲ ▼ mover • OK elegir). Tu elección queda guardada para este episodio.";

  const listEl = document.getElementById("sub-modal-list");
  listEl.innerHTML = "";

  if (!options.length) {
    listEl.innerHTML =
      '<p style="color:#aaa; padding:20px; text-align:center;">No se encontraron subtítulos en español para este episodio.</p>';
  }

  options.forEach(function (opt) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "source-item-card";
    const pctClass = opt.score >= 80 ? "high" : "";
    const tags =
      '<span class="source-tag">' + escHtml(opt.provider) + "</span>" +
      '<span class="source-tag">' + escHtml(String(opt.format || "").toUpperCase()) + "</span>" +
      '<span class="source-tag">' + opt.cues + " líneas</span>" +
      (opt.exact_file ? '<span class="source-tag ok">✔ Coincide con el archivo</span>' : "");
    const notes = (opt.notes || []).length
      ? '<div class="sub-preview warn">' + escHtml(opt.notes.join(" • ")) + "</div>"
      : "";
    const preview = (opt.preview || []).length
      ? '<div class="sub-preview">' + opt.preview.map(escHtml).join("<br>") + "</div>"
      : "";
    btn.innerHTML =
      '<div class="source-score-pill ' + pctClass + '">' + opt.score + "%</div>" +
      '<div class="source-item-info">' +
      '<div class="source-item-title">' + escHtml(opt.label) + "</div>" +
      '<div class="source-item-tags">' + tags + "</div>" + notes + preview +
      "</div>";
    btn.addEventListener("click", function () { applySubtitleOption(opt); });
    listEl.appendChild(btn);
  });

  const aiBtn = document.createElement("button");
  aiBtn.type = "button";
  aiBtn.className = "source-item-card";
  aiBtn.innerHTML =
    '<div class="source-item-info"><div class="source-item-title">🤖 Traducir con IA (inglés → español)</div>' +
    '<div class="sub-preview">Toma el subtítulo en inglés hecho para tu archivo y lo traduce. ' +
    "Tarda 30-60 s la primera vez; después queda guardado para este episodio.</div></div>";
  aiBtn.addEventListener("click", translateWithAi);
  listEl.appendChild(aiBtn);

  const reset = document.createElement("button");
  reset.type = "button";
  reset.className = "source-item-card";
  reset.innerHTML =
    '<div class="source-item-info"><div class="source-item-title">↺ Volver a selección automática</div>' +
    '<div class="sub-preview">Olvida tu elección guardada para este episodio.</div></div>';
  reset.addEventListener("click", resetSubtitleChoice);
  listEl.appendChild(reset);

  modal.classList.remove("hidden");
  updateSubModalFocus();
}

function updateSubModalFocus() {
  const items = document.querySelectorAll("#sub-modal-list .source-item-card");
  if (items.length === 0) {
    document.getElementById("sub-modal-close-btn")?.classList.add("focused");
    return;
  }
  subModalIndex = Math.max(0, Math.min(subModalIndex, items.length - 1));
  items.forEach((el, i) => el.classList.toggle("focused", i === subModalIndex));
  items[subModalIndex].scrollIntoView({ behavior: "smooth", block: "nearest" });
}

function closeSubtitleModal() {
  const modal = document.getElementById("cr-sub-modal");
  if (modal) modal.classList.add("hidden");
  activeSubModalContext = null;
  updateFocus();
}

async function translateWithAi() {
  if (!activeSubModalContext) return;
  const { anime, epNum, startMs } = activeSubModalContext;
  closeSubtitleModal();
  const attemptId = ++currentPlayAttemptId;
  isSearching = true;
  showLoadingEvent(anime, epNum);
  const statusEl = document.getElementById("load-status");
  if (statusEl) statusEl.textContent = "Buscando el subtítulo en inglés...";
  try {
    const opt = await invoke("ai_translate_subtitle", {
      anilistId: anime.id || 0,
      title: anime.title || anime.title_romaji || "",
      episode: epNum,
    });
    // Si el usuario canceló mientras tanto, la traducción igual queda guardada para la próxima
    if (attemptId !== currentPlayAttemptId) return;
    activeSubModalContext = { anime, epNum, startMs, options: [] };
    await applySubtitleOption(opt);
  } catch (err) {
    if (attemptId !== currentPlayAttemptId) return;
    hideLoadingEvent();
    showTvAlert("No se pudo traducir", String(err || "Error desconocido"));
  } finally {
    if (attemptId === currentPlayAttemptId) isSearching = false;
  }
}

async function applySubtitleOption(opt) {
  if (!activeSubModalContext || !opt) return;
  const { anime, epNum, startMs } = activeSubModalContext;
  closeSubtitleModal();
  showLoadingEvent(anime, epNum);
  const statusEl = document.getElementById("load-status");
  if (statusEl) statusEl.textContent = "Aplicando subtítulo...";
  try {
    await invoke("apply_subtitle_choice", {
      anilistId: anime.id || 0,
      episode: epNum,
      option: opt,
      startMs: startMs || 0,
    });
    // El reproductor ya se abrió (o se aplicó al MPV): la pantalla de carga debe cerrarse siempre.
    // Si queda visible, al volver del reproductor las teclas caen en el catálogo de abajo.
    hideLoadingEvent();
  } catch (err) {
    hideLoadingEvent();
    showTvAlert("No se pudo aplicar", String(err || "Error desconocido"));
  }
}

async function resetSubtitleChoice() {
  if (!activeSubModalContext) return;
  const { anime, epNum } = activeSubModalContext;
  closeSubtitleModal();
  try {
    await invoke("clear_subtitle_choice_cmd", { anilistId: anime.id || 0, episode: epNum });
    showTvAlert("Listo", "Se usará la selección automática la próxima vez que abras este episodio.");
  } catch (err) {
    showTvAlert("Error", String(err));
  }
}

/* =====================================================================
   HISTORIAL LOCAL Y RECOMENDACIONES
   Todo vive en localStorage (≈1 KB por anime, máximo 60 animes).
   ===================================================================== */
const HISTORY_KEY = "cr_history_v1";
const HISTORY_MAX = 60;
const RECO_CACHE_KEY = "cr_reco_v1";
const RECO_TTL_MS = 6 * 60 * 60 * 1000;
let historyMode = "viendo"; // "viendo" | "vistos"
let RECO_LIST = [];

/* El WebView guarda localStorage con retraso y, si el proceso muere justo al abrir el reproductor,
   puede perder lo último. Por eso el historial y "ahora reproduciendo" se espejan en un archivo
   nativo (escritura inmediata) y se combinan al leer. */
function nativeWrite(name, value) {
  try {
    if (window.AndroidExo && typeof window.AndroidExo.writeKv === "function") {
      window.AndroidExo.writeKv(name, value);
    }
  } catch (_) { }
}

function nativeRead(name) {
  try {
    if (window.AndroidExo && typeof window.AndroidExo.readKv === "function") {
      return window.AndroidExo.readKv(name) || "";
    }
  } catch (_) { }
  return "";
}

function parseObj(raw) {
  try {
    const o = JSON.parse(raw || "{}");
    return o && typeof o === "object" ? o : {};
  } catch (_) {
    return {};
  }
}

function loadHistory() {
  let local = {};
  try { local = parseObj(localStorage.getItem(HISTORY_KEY)); } catch (_) { }
  const nat = parseObj(nativeRead("history"));
  const out = Object.assign({}, local);
  Object.keys(nat).forEach(function (k) {
    const n = nat[k];
    const l = out[k];
    if (!l || (n.updatedAt || 0) > (l.updatedAt || 0)) {
      out[k] = n;
    } else if (n.resume && (!l.resume || (n.resume.at || 0) > (l.resume.at || 0))) {
      l.resume = n.resume;
    }
  });
  return out;
}

function saveHistory(h) {
  const json = JSON.stringify(h);
  try { localStorage.setItem(HISTORY_KEY, json); } catch (_) { }
  nativeWrite("history", json);
}

function slimAnime(a) {
  return {
    id: a.id || 0,
    title: a.title || "",
    title_romaji: a.title_romaji || "",
    synonyms: (a.synonyms || []).slice(0, 6),
    cover: a.cover || "",
    banner: a.banner || "",
    desc: String(a.desc || "").slice(0, 320),
    genres: (a.genres || []).slice(0, 5),
    total_episodes: a.total_episodes || 0,
    season_year: a.season_year || null,
    ep: "",
  };
}

function isCompleted(entry) {
  const total = entry && entry.anime ? entry.anime.total_episodes || 0 : 0;
  return total > 0 && entry.lastEp >= total;
}

function recordWatch(anime, ep, maxEp) {
  if (!anime || !anime.id || !ep) return;
  try {
    const h = loadHistory();
    const key = String(anime.id);
    const prev = h[key] || {};
    const seen = Array.isArray(prev.seen) ? prev.seen.slice() : [];
    if (seen.indexOf(ep) < 0) seen.push(ep);
    h[key] = {
      anime: slimAnime(anime),
      lastEp: ep,
      maxEp: Math.max(maxEp || 0, ep, prev.maxEp || 0),
      seen: seen.slice(-400),
      updatedAt: Date.now(),
      resume: prev.resume || null,
    };
    const keys = Object.keys(h);
    if (keys.length > HISTORY_MAX) {
      keys
        .sort(function (a, b) { return h[a].updatedAt - h[b].updatedAt; })
        .slice(0, keys.length - HISTORY_MAX)
        .forEach(function (k) { delete h[k]; });
    }
    saveHistory(h);
    console.log("[CR hist] guardado " + key + " ep " + ep);
    try { localStorage.removeItem(RECO_CACHE_KEY); } catch (_) { } // el perfil cambió
  } catch (_) { }
}

function historyEntriesSorted() {
  return Object.values(loadHistory()).sort(function (a, b) { return b.updatedAt - a.updatedAt; });
}

function fillListGrid(gridId, list, emptyHtml) {
  const grid = document.getElementById(gridId);
  if (!grid) return;
  grid.innerHTML = list.length > 0 ? buildCardsHtml(list, true) : emptyHtml;
  bindCardClicks(grid, list);
  grid.scrollTop = 0;
}

function emptyStateHtml(title, text) {
  return (
    '<div class="empty-state"><div class="empty-title">' + title + "</div><p>" + text + "</p></div>"
  );
}

/* ── Mi lista: Viendo / Vistos ── */
function renderHistorialTab(restoreIdx) {
  const entries = historyEntriesSorted();
  const watching = entries.filter(function (e) { return !isCompleted(e); });
  const done = entries.filter(isCompleted);

  const pills = document.getElementById("historial-pills");
  if (pills) {
    pills.innerHTML = "";
    [["viendo", "Viendo (" + watching.length + ")"], ["vistos", "Vistos (" + done.length + ")"]].forEach(function (p) {
      const b = document.createElement("button");
      b.className = "range-pill" + (historyMode === p[0] ? " active" : "");
      b.textContent = p[1];
      b.addEventListener("click", function () {
        historyMode = p[0];
        filterIndex = p[0] === "vistos" ? 1 : 0;
        renderHistorialTab();
        currentArea = "filters";
        updateFocus();
      });
      pills.appendChild(b);
    });
  }

  const source = historyMode === "vistos" ? done : watching;
  const list = source.map(function (e) {
    const label = historyMode === "vistos"
      ? "✔ Completado"
      : "Ep. " + e.lastEp +
        (e.anime.total_episodes ? " de " + e.anime.total_episodes : "") +
        (e.resume && e.resume.ep === e.lastEp && e.resume.pos >= 30000 ? " • " + formatMs(e.resume.pos) : "");
    return Object.assign({}, e.anime, { _label: label });
  });
  ANIMES = list;
  fillListGrid(
    "historial-grid",
    list,
    emptyStateHtml(
      historyMode === "vistos" ? "Aún no terminas ningún anime" : "Nada en progreso",
      historyMode === "vistos"
        ? "Los animes que llegues a su último episodio aparecerán aquí."
        : "Empieza a ver un anime y aparecerá aquí para continuarlo rápido."
    )
  );
  cardIndex = typeof restoreIdx === "number" ? Math.min(restoreIdx, Math.max(0, list.length - 1)) : 0;
  currentArea = list.length > 0 ? "cards" : "filters";
  if (currentArea === "filters") filterIndex = historyMode === "vistos" ? 1 : 0;
  updateFocus();
}

/* ── Para ti: recomendaciones por géneros de lo visto ── */
async function computeRecommendations() {
  const h = loadHistory();
  const entries = Object.values(h);
  const seenIds = new Set(Object.keys(h));
  const now = Date.now();
  const weights = {};
  entries.forEach(function (e) {
    const ageDays = (now - e.updatedAt) / 86400000;
    const w =
      (1 + Math.max(0, 1 - ageDays / 60)) *
      (isCompleted(e) ? 1.4 : 1) *
      (1 + Math.min((e.seen || []).length, 12) / 12);
    (e.anime.genres || []).forEach(function (g) { weights[g] = (weights[g] || 0) + w; });
  });
  const top = Object.entries(weights)
    .sort(function (a, b) { return b[1] - a[1]; })
    .slice(0, 3)
    .map(function (x) { return x[0]; });

  const sig = top.join("|") + ":" + entries.length;
  try {
    const cached = JSON.parse(localStorage.getItem(RECO_CACHE_KEY) || "null");
    if (cached && cached.sig === sig && now - cached.at < RECO_TTL_MS) return cached;
  } catch (_) { }

  const calls = [];
  if (top.length === 0) {
    calls.push(invoke("get_catalog_page", { page: 1, genre: "ALL", sort: "TRENDING", year: null, format: null }));
  } else {
    top.slice(0, 2).forEach(function (g) {
      calls.push(invoke("get_catalog_page", { page: 1, genre: g, sort: "SCORE", year: null, format: null }));
    });
    calls.push(invoke("get_catalog_page", { page: 1, genre: top[0], sort: "POPULAR", year: null, format: null }));
  }
  const results = await Promise.allSettled(calls);

  const byId = {};
  results.forEach(function (r) {
    if (r.status !== "fulfilled" || !Array.isArray(r.value)) return;
    r.value.forEach(function (a, pos) {
      if (!a || !a.id || seenIds.has(String(a.id))) return;
      let score = 0;
      (a.genres || []).forEach(function (g) { score += weights[g] || 0; });
      score += Math.max(0, 25 - pos) / 25; // pequeño bonus por posición
      if (byId[a.id]) byId[a.id].score += 1.5; // aparece en varias listas
      else byId[a.id] = { anime: a, score: score };
    });
  });

  const list = Object.values(byId)
    .sort(function (a, b) { return b.score - a.score; })
    .slice(0, 30)
    .map(function (x) { return slimAnime(x.anime); });

  const out = { sig: sig, at: now, top: top, list: list };
  try {
    localStorage.setItem(RECO_CACHE_KEY, JSON.stringify(out));
  } catch (_) { }
  return out;
}

async function openRecoTab(restoreIdx) {
  const grid = document.getElementById("reco-grid");
  const sub = document.getElementById("reco-sub");
  if (typeof restoreIdx === "number" && RECO_LIST.length > 0) {
    ANIMES = RECO_LIST;
    fillListGrid("reco-grid", RECO_LIST, "");
    cardIndex = Math.min(restoreIdx, RECO_LIST.length - 1);
    currentArea = "cards";
    updateFocus();
    return;
  }
  if (grid) grid.innerHTML = '<p class="loading-note">Preparando recomendaciones…</p>';
  ANIMES = [];
  currentArea = "sidebar";
  updateFocus();
  try {
    const res = await computeRecommendations();
    if (activeTab !== "recomendados") return;
    RECO_LIST = res.list || [];
    if (sub) {
      sub.textContent = res.top && res.top.length
        ? "Basado en lo que ves: " + res.top.join(" • ")
        : "Aún no hay historial: te muestro lo que está en tendencia";
    }
    ANIMES = RECO_LIST;
    fillListGrid(
      "reco-grid",
      RECO_LIST,
      emptyStateHtml("Sin recomendaciones por ahora", "Mira algún capítulo y vuelve: aprenderemos de tus gustos.")
    );
    cardIndex = 0;
    currentArea = RECO_LIST.length > 0 ? "cards" : "sidebar";
    updateFocus();
  } catch (err) {
    if (grid) grid.innerHTML = '<p class="loading-note">No se pudieron cargar las recomendaciones: ' + escHtml(err) + "</p>";
  }
}

/* =====================================================================
   VUELTA DESDE EL REPRODUCTOR: minuto guardado y "siguiente episodio"
   ===================================================================== */
function applyProgressRaw(raw) {
  // formato: "animeId|episodio|posMs|durMs"
  try {
    const parts = String(raw || "").split("|");
    if (parts.length < 4) return;
    const key = parts[0];
    const h = loadHistory();
    if (!h[key]) return;
    h[key].resume = {
      ep: parseInt(parts[1], 10) || 0,
      pos: parseInt(parts[2], 10) || 0,
      dur: parseInt(parts[3], 10) || 0,
      at: Date.now(),
    };
    saveHistory(h);
    console.log("[CR hist] minuto " + key + " ep " + parts[1] + " pos " + parts[2]);
  } catch (_) { }
}

function getResumeMs(animeId, ep) {
  try {
    const e = loadHistory()[String(animeId)];
    const r = e && e.resume;
    if (!r || r.ep !== ep || r.pos < 30000) return 0;
    if (r.dur > 0 && r.pos > r.dur - 60000) return 0; // casi terminado: empieza de cero
    return Math.max(0, r.pos - 3000);
  } catch (_) {
    return 0;
  }
}

function formatMs(ms) {
  const t = Math.floor(ms / 1000);
  const m = Math.floor(t / 60);
  const s = t % 60;
  return m + ":" + (s < 10 ? "0" : "") + s;
}

// Log visible en logcat (etiqueta CR_TV) además de la consola
function crLog(msg) {
  try { console.log("[CR play] " + msg); } catch (_) { }
  try {
    if (window.AndroidExo && typeof window.AndroidExo.log === "function") window.AndroidExo.log(String(msg));
  } catch (_) { }
}

function playEpisodeFromPlayer(ep) {
  // Tras volver del reproductor se ignoran teclas hasta que empiece la búsqueda del nuevo episodio:
  // una pulsación suelta del control remoto no debe activar botones del catálogo (p. ej. "Ver Episodio 1")
  window.__crInputGuardUntil = Date.now() + 4000;
  window.__crLogKeysUntil = Date.now() + 10000;
  const np = readNowPlaying();
  crLog("playEpisodeFromPlayer ep=" + ep + " np.ep=" + (np ? np.ep : "null") + " np.max=" + (np ? np.max : "null"));
  if (!np || !np.anime) return;
  // Pantalla de carga inmediata: evita ver el catálogo mientras se busca el nuevo episodio
  try { showLoadingEvent(np.anime, ep, true); } catch (_) { }
  setTimeout(function () {
    iniciarReproduccion(np.anime, ep, np.max || ep);
  }, 300);
}

window.crHandleReturnFromPlayer = function () {
  const ex = window.AndroidExo;
  if (!ex) return;
  // Red de seguridad: si quedó una pantalla de carga huérfana, se cierra al volver del reproductor
  try {
    const ov = document.getElementById("loading-overlay");
    if (!isSearching && ov && ov.style.display !== "none") hideLoadingEvent();
  } catch (_) { }
  try {
    if (typeof ex.consumeProgress === "function") {
      const raw = ex.consumeProgress();
      if (raw) applyProgressRaw(raw);
    }
  } catch (_) { }
  try {
    if (typeof ex.consumePlayEpFlag === "function") {
      const ep = ex.consumePlayEpFlag();
      crLog("crHandleReturnFromPlayer flag=" + ep + " isSearching=" + isSearching);
      if (ep > 0) playEpisodeFromPlayer(ep);
    }
  } catch (_) { }
};

// ── Episodio en reproducción (persistente) ─────────────────────────────
// Se guarda al iniciar cada episodio. openAnimeDetail() reescribe
// "cr_last_detail_anime" con lastEp:1, así que no sirve para saber qué cap. se está viendo.
const NOW_PLAYING_KEY = "cr_now_playing";
const NOW_PLAYING_MAX_AGE_MS = 12 * 60 * 60 * 1000;

function saveNowPlaying(anime, ep, maxEp) {
  if (!anime || !ep) return;
  recordWatch(anime, ep, maxEp);
  const json = JSON.stringify({ anime: anime, ep: ep, max: maxEp || ep, savedAt: Date.now() });
  try { localStorage.setItem(NOW_PLAYING_KEY, json); } catch (_) { }
  nativeWrite("now_playing", json);
}

function readNowPlaying() {
  let best = null;
  const candidates = [];
  try { candidates.push(JSON.parse(localStorage.getItem(NOW_PLAYING_KEY) || "null")); } catch (_) { }
  try { candidates.push(JSON.parse(nativeRead("now_playing") || "null")); } catch (_) { }
  candidates.forEach(function (np) {
    if (np && np.anime && np.ep && Date.now() - (np.savedAt || 0) < NOW_PLAYING_MAX_AGE_MS) {
      if (!best || (np.savedAt || 0) > (best.savedAt || 0)) best = np;
    }
  });
  return best;
}

window.openSourcesForCurrentEpisode = function () {
  let targetAnime = null;
  let targetEp = 1;
  let targetMax = 1;

  const np = readNowPlaying();
  if (np) {
    targetAnime = np.anime;
    targetEp = np.ep;
    targetMax = np.max || np.ep;
  }

  if (!targetAnime) try {
    const detailSaved = JSON.parse(
      localStorage.getItem("cr_last_detail_anime") ||
      sessionStorage.getItem("cr_last_detail_anime") ||
      "null"
    );
    if (detailSaved && detailSaved.anime) {
      targetAnime = detailSaved.anime;
      targetEp = detailSaved.lastEp || 1;
      targetMax = detailSaved.anime.total_episodes || targetEp;
    }
  } catch (_) { }

  if (!targetAnime && activeAnime) {
    targetAnime = activeAnime;
    targetEp = activeEpisodeNum;
    targetMax = totalSeasonEpisodes;
  }

  if (targetAnime) {
    openManualSourcesForDetail(targetAnime, targetEp, targetMax);
  }
};


async function openManualSourcesForDetail(anime, epNum, maxEp) {
  if (!anime || isSearching) return;
  const attemptId = ++currentPlayAttemptId;
  isSearching = true;
  showLoadingEvent(anime, epNum);
  const statusEl = document.getElementById("load-status");
  if (statusEl) statusEl.textContent = "Buscando todas las fuentes disponibles...";

  try {
    const candidates = await invoke("search_episode_sources", {
      title: anime.title,
      titleRomaji: anime.title_romaji,
      synonyms: anime.synonyms || [],
      episode: epNum,
      year: anime.season_year || null, // <-- AQUÍ VA ESTA LÍNEA
    });
    if (attemptId !== currentPlayAttemptId) return;

    openSourceSelectorModal(
      anime,
      epNum,
      maxEp,
      candidates,
      "Selecciona el release óptimo (quedará guardado en caché):"
    );
  } catch (err) {
    if (attemptId !== currentPlayAttemptId) return;
    hideLoadingEvent();
    showTvAlert("Sin fuentes", String(err || "No se encontraron releases."));
  } finally {
    if (attemptId === currentPlayAttemptId) isSearching = false;
  }
}

async function confirmManualSourceSelection(cand) {
  if (!activeSourceModalContext || !cand) return;
  const { anime, epNum, maxKnownEp } = activeSourceModalContext;
  closeSourceSelectorModal();

  const attemptId = ++currentPlayAttemptId;
  isSearching = true;
  currentPlayingFullTitle = anime.title + " — Episodio " + epNum;
  saveNowPlaying(anime, epNum, maxKnownEp);
  showLoadingEvent(anime, epNum);

  try {
    await invoke("play_and_save_source", {
      anilistId: anime.id || 0,
      title: anime.title,
      titleRomaji: anime.title_romaji,
      synonyms: anime.synonyms || [],
      episode: epNum,
      magnet: cand.magnet,
      infoHash: cand.info_hash,
      releaseTitle: cand.title,
      spanishScore: cand.spanish_score,
    });

    if (attemptId !== currentPlayAttemptId) return;
    activeAnime = anime;
    activeEpisodeNum = epNum;
    totalSeasonEpisodes = maxKnownEp || epNum;
    openPlayerOverlay(anime.title, epNum);
  } catch (err) {
    if (attemptId !== currentPlayAttemptId) return;
    hideLoadingEvent();
    showTvAlert("Error con la fuente elegida", String(err || ""));
  } finally {
    if (attemptId === currentPlayAttemptId) {
      hideLoadingEvent();
      isSearching = false;
    }
  }
}


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
function showLoadingEvent(animeOrTitle, epNum, immediate) {
  // Si ya había una pantalla de carga en marcha, apagamos sus temporizadores: si no, quedan
  // temporizadores "huérfanos" que reabren la pantalla y pelean entre sí (contador que sube y baja).
  if (loadDelayTimer) {
    clearTimeout(loadDelayTimer);
    loadDelayTimer = null;
  }
  if (loadSecondsInterval) {
    clearInterval(loadSecondsInterval);
    loadSecondsInterval = null;
  }
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
  const revealLoading = function () {
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
  };
  // "immediate": al volver del reproductor se muestra al instante, sin dejar ver el catálogo
  if (immediate) {
    revealLoading();
  } else {
    loadDelayTimer = setTimeout(revealLoading, 300);
  }
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

  {
    const order = Array.from(document.querySelectorAll(".nav-btn")).map(function (b) { return b.dataset.tab; });
    const found = order.indexOf(tabName);
    if (found >= 0) sideIndex = found;
  }

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
  } else if (tabName === "recomendados") {
    openRecoTab(restoreCardIdx);
  } else if (tabName === "historial") {
    renderHistorialTab(restoreCardIdx);
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
  try {
    const payload = JSON.stringify({
      anime: anime,
      previousTab: previousTabBeforeDetail,
      savedCardIndex: savedCardIndexBeforeDetail,
      lastEp: 1
    });
    sessionStorage.setItem("cr_last_detail_anime", payload);
    localStorage.setItem("cr_last_detail_anime", payload);
  } catch (_) { }
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
  const sourcesBtn = document.getElementById("detail-sources-btn");
  playFirstBtn.textContent = "▶ Ver Episodio 1";
  playLatestBtn.textContent = "⚡ Ver Último (Ep. " + maxEp + ")";
  let firstEpTarget = 1;
  try {
    const hist = anime.id ? loadHistory()[String(anime.id)] : null;
    if (hist && hist.lastEp > 0 && hist.lastEp <= maxEp) {
      firstEpTarget = hist.lastEp;
      const rMs = getResumeMs(anime.id, hist.lastEp);
      playFirstBtn.textContent = "▶ Continuar Ep. " + hist.lastEp + (rMs > 0 ? " (" + formatMs(rMs) + ")" : "");
    }
  } catch (_) { }
  playFirstBtn.onclick = function () {
    iniciarReproduccion(anime, firstEpTarget, maxEp);
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
    const subLabel = anime._label
      ? anime._label
      : isCatalogMode
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

function getFilterItems() {
  if (activeTab === "historial") {
    return Array.from(document.querySelectorAll("#historial-pills .range-pill"));
  }
  return Array.from(
    document.querySelectorAll("#view-catalogo .cr-select, #catalog-range-pills .range-pill")
  );
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
      const rangePills = document.querySelectorAll("#detail-range-pills .range-pill");
      if (rangePills.length > 0) {
        detailRangeIndex = Math.max(0, Math.min(detailRangeIndex, rangePills.length - 1));
        rangePills[detailRangeIndex].classList.add("focused");
        rangePills[detailRangeIndex].scrollIntoView({ behavior: "auto", inline: "center", block: "nearest" });
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
  } else if (currentArea === "filters" && (activeTab === "catalogo" || activeTab === "historial")) {
    const filterItems = getFilterItems();
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
  try {
    crLog("iniciarReproduccion custom=" + customEp + " max=" + maxKnownEp +
      " isSearching=" + isSearching + " activeEp=" + activeEpisodeNum +
      " from=" + String(new Error().stack || "").split("\n").slice(2, 5).map(function (s) { return s.trim(); }).join(" <- "));
  } catch (_) { }
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

  currentPlayingFullTitle = anime.title + " — Episodio " + epToPlay;
  saveNowPlaying(anime, epToPlay, latestEp);
  const epTitleEl = document.getElementById("osd-episode-title");
  if (epTitleEl) epTitleEl.textContent = currentPlayingFullTitle;

  try {
    localStorage.setItem("cr_last_detail_anime", JSON.stringify({
      anime: anime,
      previousTab: previousTabBeforeDetail,
      savedCardIndex: savedCardIndexBeforeDetail,
      lastEp: epToPlay,
      savedAt: Date.now()
    }));
  } catch (_) { }


  showLoadingEvent(anime, epToPlay);

  try {
    const res = await invoke("play_anime_episode", {
      title: anime.title,
      titleRomaji: anime.title_romaji,
      synonyms: anime.synonyms || [],
      episode: epToPlay,
      anilistId: anime.id || 0,
      year: anime.season_year || null, // <-- AQUÍ VA LA SEGUNDA LÍNEA
    });

    if (attemptId !== currentPlayAttemptId) return;


    // Si Rust detectó puntaje < 75% o sin caché directo, abre el selector automáticamente
    if (res && res.status === "select_source") {
      openSourceSelectorModal(anime, epToPlay, latestEp, res.candidates || [], res.reason);
      return;
    }

    activeAnime = anime;

    try {
      const savedRaw = sessionStorage.getItem("cr_last_detail_anime");
      const savedObj = savedRaw ? JSON.parse(savedRaw) : { anime: anime, previousTab: previousTabBeforeDetail, savedCardIndex: savedCardIndexBeforeDetail };
      savedObj.anime = anime;
      savedObj.lastEp = epToPlay;
      sessionStorage.setItem("cr_last_detail_anime", JSON.stringify(savedObj));
    } catch (_) { }
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
  if (Date.now() < (window.__crLogKeysUntil || 0)) {
    crLog("key=" + e.key + " tab=" + activeTab + " detail=" + detailSection + " isSearching=" + isSearching +
      " guard=" + (Date.now() < (window.__crInputGuardUntil || 0)));
  }
  if (!isSearching && Date.now() < (window.__crInputGuardUntil || 0)) {
    e.preventDefault();
    e.stopPropagation();
    return;
  }
  if (activeTvSelectEl) {
    const items = document.querySelectorAll("#cr-tv-dropdown-list .cr-tv-dropdown-item");
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (tvSelectOptionIndex < items.length - 1) tvSelectOptionIndex++;
      updateTvDropdownFocus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (tvSelectOptionIndex > 0) tvSelectOptionIndex--;
      updateTvDropdownFocus();
    } else if (e.key === "Enter") {
      e.preventDefault();
      items[tvSelectOptionIndex]?.click();
    } else if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      closeTvSelectDropdown();
    }
    return;
  }
  if (activeSubModalContext) {
    const items = document.querySelectorAll("#sub-modal-list .source-item-card");
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (subModalIndex < items.length - 1) subModalIndex++;
      updateSubModalFocus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (subModalIndex > 0) subModalIndex--;
      updateSubModalFocus();
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (items.length > 0) items[subModalIndex]?.click();
      else closeSubtitleModal();
    } else if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      closeSubtitleModal();
    }
    return;
  }
  if (activeSourceModalContext) {
    const items = document.querySelectorAll("#source-modal-list .source-item-card");
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (sourceModalIndex < items.length - 1) sourceModalIndex++;
      updateSourceModalFocus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (sourceModalIndex > 0) sourceModalIndex--;
      updateSourceModalFocus();
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (items.length > 0) items[sourceModalIndex]?.click();
      else closeSourceSelectorModal();
    } else if (e.key === "Escape" || e.key === "Backspace" || e.key === "GoBack") {
      e.preventDefault();
      closeSourceSelectorModal();
    }
    return;
  }


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
        const actionBtns = [
          document.getElementById("detail-play-first"),
          document.getElementById("detail-play-latest"),
          document.getElementById("detail-sources-btn"),
        ].filter(Boolean);
        actionBtns[detailActionIndex]?.click();
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
        const nSelUp = document.querySelectorAll("#view-catalogo .cr-select").length;
        filterIndex = nSelUp + Math.max(0, catalogPage - 1);
      } else if (activeTab === "historial") {
        currentArea = "filters";
        filterIndex = historyMode === "vistos" ? 1 : 0;
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
    const filterItems = getFilterItems();
    // En Catálogo hay 2 filas: los selectores y, debajo, las cápsulas de páginas (1-25, 26-50…)
    const nSel = activeTab === "catalogo" ? document.querySelectorAll("#view-catalogo .cr-select").length : 0;
    const inPills = activeTab === "catalogo" && filterIndex >= nSel;
    const rowStart = inPills ? nSel : 0;
    const rowEnd = activeTab === "catalogo" && !inPills ? nSel - 1 : filterItems.length - 1;
    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (filterIndex < rowEnd) filterIndex++;
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (filterIndex > rowStart) filterIndex--;
      else currentArea = "sidebar";
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (activeTab === "catalogo" && !inPills && filterItems.length > nSel) {
        filterIndex = Math.min(filterItems.length - 1, nSel + Math.max(0, catalogPage - 1));
      } else if (ANIMES.length > 0) {
        currentArea = "cards";
      }
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (inPills) filterIndex = 0;
    } else if (e.key === "Enter" && filterItems.at(filterIndex)) {
      e.preventDefault();
      const targetEl = filterItems.at(filterIndex);
      if (targetEl.tagName === "SELECT") {
        openTvSelectDropdown(targetEl);
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
      } else if (activeTab === "catalogo" || activeTab === "historial") {
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

  // Restaurar ficha del anime de inmediato si volvemos del reproductor en TV
  let restoredDetailOnBoot = false;
  try {
    const savedRaw = localStorage.getItem("cr_last_detail_anime");
    if (savedRaw) {
      const savedObj = JSON.parse(savedRaw);
      const isFresh = savedObj && savedObj.savedAt && (Date.now() - savedObj.savedAt < 2 * 60 * 60 * 1000);
      if (isFresh && savedObj.anime) {
        previousTabBeforeDetail = savedObj.previousTab || "inicio";
        savedCardIndexBeforeDetail = savedObj.savedCardIndex || 0;
        openAnimeDetail(savedObj.anime);
        restoredDetailOnBoot = true;
        if (savedObj.lastEp > 1) {
          detailSection = "grid";
          const epCards = Array.from(document.querySelectorAll("#detail-episodes-grid .detail-ep-card"));
          const foundIdx = epCards.findIndex((c) => parseInt(c.dataset.ep, 10) === savedObj.lastEp);
          if (foundIdx >= 0) detailGridIndex = foundIdx;
          updateFocus();
        }
      } else {
        localStorage.removeItem("cr_last_detail_anime");
      }
    }
  } catch (_) { }

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
    if (!restoredDetailOnBoot) {
      updateFocus();
    }
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
      if (window.AndroidExo && data.url) {
        const titleEl = document.getElementById("osd-episode-title");
        const domTitle = titleEl && titleEl.textContent ? titleEl.textContent.trim() : "";
        const epTitle = currentPlayingFullTitle || domTitle;
        const npNow = readNowPlaying();
        const animeIdNow = npNow && npNow.anime ? npNow.anime.id || 0 : 0;
        const epNow = parseInt(data.episode, 10) || (npNow ? npNow.ep : 0) || 0;
        const startAt = data.startMs > 0 ? Math.floor(data.startMs) : animeIdNow ? getResumeMs(animeIdNow, epNow) : 0;
        if (window.AndroidExo.playEpisode && epNow > 0) {
          window.AndroidExo.playEpisode(
            data.url, data.subtitlePath || "", epTitle, startAt,
            epNow, npNow ? npNow.max || 0 : 0, animeIdNow ? String(animeIdNow) : ""
          );
        } else if (startAt > 0 && window.AndroidExo.playWithTitleAt) {
          window.AndroidExo.playWithTitleAt(data.url, data.subtitlePath || "", epTitle, startAt);
        } else if (window.AndroidExo.playWithTitle) {
          window.AndroidExo.playWithTitle(data.url, data.subtitlePath || "", epTitle);
        } else {
          window.AndroidExo.play(data.url, data.subtitlePath || "");
        }
      }
    });

    tauriEvent.listen("ai-sub-progress", function (event) {
      const p = event.payload || {};
      const el = document.getElementById("load-status");
      if (!el) return;
      el.textContent = (p.phase || "Traduciendo con IA...") + (p.total ? " (" + (p.done || 0) + "/" + p.total + " partes)" : "");
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
  if (typeof window.crHandleReturnFromPlayer === "function") window.crHandleReturnFromPlayer();
  if (window.AndroidExo && typeof window.AndroidExo.consumeOpenSubsFlag === "function") {
    const subsMs = window.AndroidExo.consumeOpenSubsFlag();
    if (subsMs >= 0) {
      setTimeout(() => {
        if (typeof window.openSubtitlePickerForCurrentEpisode === "function") {
          window.openSubtitlePickerForCurrentEpisode(subsMs);
        }
      }, 300);
    }
  }
  if (window.AndroidExo && typeof window.AndroidExo.consumeOpenSourcesFlag === "function") {
    if (window.AndroidExo.consumeOpenSourcesFlag()) {
      setTimeout(() => {
        if (typeof window.openSourcesForCurrentEpisode === "function") {
          window.openSourcesForCurrentEpisode();
        }
      }, 300);
    }
  }
});