pub const ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="14" fill="#181825"/>
  <circle cx="32" cy="32" r="21" fill="none" stroke="#89dceb" stroke-width="4"/>
  <polygon points="27,22 45,32 27,42" fill="#a6e3a1"/>
</svg>
"##;

pub const MANIFEST_JSON: &str = r##"{
  "name": "rust-audio-p2p",
  "short_name": "rust-audio",
  "start_url": "/",
  "display": "standalone",
  "background_color": "#11111b",
  "theme_color": "#181825",
  "icons": [
    {
      "src": "/icon.svg",
      "sizes": "any",
      "type": "image/svg+xml"
    }
  ]
}
"##;

pub const INDEX_HTML: &str = r##"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="theme-color" content="#181825" />
  <title>rust-audio-p2p — Web / PWA</title>
  <link rel="manifest" href="/manifest.json" />
  <link rel="icon" type="image/svg+xml" href="/icon.svg" />
  <style>
    :root {
      --bg: #11111b; --surface: #181825; --card: #1e1e2e; --border: #313244;
      --text: #cdd6f4; --muted: #a6adc8; --accent: #89dceb; --green: #a6e3a1;
      --yellow: #f9e2af; --red: #f38ba8; --mauve: #cba6f7;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0; font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
      background: var(--bg); color: var(--text); padding-bottom: 124px;
    }
    header {
      position: sticky; top: 0; z-index: 20; background: var(--surface);
      border-bottom: 1px solid var(--border); padding: 10px 14px;
      display: flex; flex-wrap: wrap; gap: 10px; align-items: center; justify-content: space-between;
    }
    .brand { display: flex; align-items: center; gap: 10px; font-weight: 700; color: var(--accent); }
    .brand img { width: 26px; height: 26px; border-radius: 6px; }
    .pill-row { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
    button, input, select {
      background: var(--card); color: var(--text); border: 1px solid var(--border);
      border-radius: 8px; padding: 7px 11px; font-size: 13px;
    }
    button { cursor: pointer; font-weight: 600; transition: border-color .15s, background .15s; }
    button:hover { border-color: var(--accent); }
    button.active { background: var(--accent); color: #11111b; border-color: var(--accent); }
    button.green { border-color: var(--green); color: var(--green); }
    button.danger { border-color: var(--red); color: var(--red); }
    nav.tabs {
      display: flex; gap: 6px; padding: 10px 14px; background: var(--bg);
      border-bottom: 1px solid var(--border); overflow-x: auto;
    }
    main { max-width: 1120px; margin: 0 auto; padding: 14px; }
    .panel {
      background: var(--surface); border: 1px solid var(--border);
      border-radius: 10px; padding: 12px; margin-bottom: 12px;
    }
    .batch-bar {
      background: #232338; border: 1px solid var(--accent);
      border-radius: 10px; padding: 8px 12px; margin-bottom: 12px;
      justify-content: space-between;
    }
    .row { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
    .grow { flex: 1; min-width: 180px; }
    .track-list { display: grid; gap: 6px; margin-top: 10px; }
    .track-item {
      display: grid; grid-template-columns: 24px 30px 1fr auto; gap: 8px; align-items: center;
      background: var(--card); border: 1px solid transparent; border-radius: 8px; padding: 8px 10px;
      cursor: pointer;
    }
    .track-chk { width: 16px; height: 16px; accent-color: var(--accent); cursor: pointer; margin: 0; }
    .track-item:hover { border-color: var(--border); }
    .track-item.playing { border-color: var(--accent); background: #232338; }
    .track-meta { min-width: 0; }
    .track-title { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .track-sub { font-size: 12px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .track-actions { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; justify-content: flex-end; }
    .badge {
      font-size: 11px; padding: 2px 6px; border-radius: 6px; background: #26263a; color: var(--muted);
      text-transform: uppercase;
    }
    .badge.peer { color: var(--accent); border: 1px solid #3b5366; text-transform: none; }
    .pin-btn.pinned { color: var(--yellow); border-color: var(--yellow); }
    .grid-2 { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 12px; }
    .player-bar {
      position: fixed; bottom: 0; left: 0; right: 0; z-index: 30;
      background: var(--surface); border-top: 1px solid var(--border);
      padding: 10px 14px; display: grid; gap: 6px;
    }
    .player-top { display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap; }
    .now-playing { display: flex; align-items: center; gap: 10px; min-width: 0; flex: 1; }
    .now-playing img { width: 42px; height: 42px; border-radius: 6px; object-fit: cover; background: var(--card); }
    .player-seek { display: flex; align-items: center; gap: 10px; }
    .player-seek input[type=range] { flex: 1; padding: 0; accent-color: var(--accent); }
    .mono { font-family: ui-monospace, monospace; font-size: 12px; word-break: break-all; color: var(--accent); }
    .muted { color: var(--muted); font-size: 12px; }
    .hidden { display: none !important; }
    dialog {
      background: var(--surface); color: var(--text); border: 1px solid var(--accent);
      border-radius: 12px; padding: 18px; max-width: 440px; width: 92%;
    }
    dialog::backdrop { background: rgba(0, 0, 0, 0.7); }
  </style>
</head>
<body>
  <header>
    <div class="brand">
      <img src="/icon.svg" alt="" />
      <span>rust-audio-p2p</span>
      <span id="nodeBadge" class="badge">LAN</span>
    </div>
    <div class="pill-row">
      <button id="modeBrowserBtn" class="active"></button>
      <button id="modeRemoteBtn"></button>
      <button id="langBtn">ES / EN</button>
      <button id="tokenBtn"></button>
    </div>
  </header>

  <nav class="tabs">
    <button class="tab-btn active" data-tab="library" id="tabLibraryBtn"></button>
    <button class="tab-btn" data-tab="search" id="tabSearchBtn"></button>
    <button class="tab-btn" data-tab="playlists" id="tabPlaylistsBtn"></button>
    <button class="tab-btn" data-tab="p2p" id="tabP2pBtn"></button>
  </nav>

  <main>
    <div id="batchBar" class="batch-bar row hidden">
      <span id="batchCount" style="font-weight:700;color:var(--accent);"></span>
      <div class="pill-row">
        <button id="selectVisibleBtn"></button>
        <button id="batchEnqueueBtn"></button>
        <button id="batchAddPlBtn" class="green"></button>
        <button id="clearSelBtn"></button>
      </div>
    </div>

    <section id="view-library" class="view">
      <div class="panel row">
        <input id="libFilter" class="grow" type="search" />
        <button id="refreshBtn"></button>
      </div>
      <div id="libList" class="track-list"></div>
    </section>

    <section id="view-search" class="view hidden">
      <div class="panel row">
        <input id="ftsInput" class="grow" type="search" />
        <button id="ftsBtn"></button>
      </div>
      <div id="searchList" class="track-list"></div>
    </section>

    <section id="view-playlists" class="view hidden">
      <div class="panel row">
        <input id="newPlInput" class="grow" type="text" />
        <button id="createPlBtn" class="green"></button>
      </div>
      <div class="grid-2">
        <div class="panel">
          <div id="plHeader" style="font-weight:700;margin-bottom:8px;"></div>
          <div id="plList" class="track-list"></div>
        </div>
        <div class="panel">
          <div class="row" style="justify-content:space-between;margin-bottom:8px;">
            <span id="plTracksHeader" style="font-weight:700;"></span>
            <div class="pill-row">
              <button id="playPlBtn" class="green"></button>
              <button id="renamePlBtn"></button>
              <button id="deletePlBtn" class="danger"></button>
            </div>
          </div>
          <div id="renamePlBar" class="row hidden" style="margin-bottom:8px;">
            <input id="renamePlInput" class="grow" type="text" />
            <button id="saveRenamePlBtn" class="green"></button>
            <button id="cancelRenamePlBtn"></button>
          </div>
          <div id="deletePlBar" class="row hidden" style="margin-bottom:8px;justify-content:space-between;">
            <span id="deletePlMsg" style="color:var(--red);font-weight:600;"></span>
            <div class="pill-row">
              <button id="confirmDeletePlBtn" class="danger"></button>
              <button id="cancelDeletePlBtn"></button>
            </div>
          </div>
          <div id="plTrackList" class="track-list"></div>
        </div>
      </div>
    </section>

    <section id="view-p2p" class="view hidden">
      <div class="panel">
        <div style="font-weight:700;margin-bottom:6px;" id="p2pNodeTitle"></div>
        <div class="mono" id="ticketUriText"></div>
        <div class="row" style="margin-top:8px;">
          <button id="copyTicketBtn"></button>
          <button id="discoverBtn"></button>
        </div>
      </div>
      <div class="panel row">
        <input id="connectPeerInput" class="grow" type="text" />
        <button id="connectPeerBtn" class="green"></button>
      </div>
      <div class="panel">
        <div id="peersHeader" style="font-weight:700;margin-bottom:8px;"></div>
        <div id="peersList" class="track-list"></div>
      </div>
    </section>
  </main>

  <footer class="player-bar">
    <div class="player-top">
      <div class="now-playing">
        <img id="coverImg" src="/icon.svg" alt="cover" />
        <div class="track-meta">
          <div class="track-title" id="nowTitle">—</div>
          <div class="track-sub" id="nowSub">rust-audio-p2p</div>
        </div>
      </div>
      <div class="pill-row">
        <button id="prevBtn">⏮</button>
        <button id="playPauseBtn">▶</button>
        <button id="nextBtn">⏭</button>
        <button id="autoNextBtn">Auto: ON</button>
        <input id="volSlider" type="range" min="0" max="1" step="0.02" value="1" style="width:82px;padding:0;" />
      </div>
    </div>
    <div class="player-seek">
      <span id="curTime" class="muted">00:00</span>
      <input id="seekSlider" type="range" min="0" max="100" step="0.5" value="0" />
      <span id="durTime" class="muted">00:00</span>
    </div>
  </footer>

  <dialog id="tokenDialog">
    <h3 id="tokenModalTitle" style="margin-top:0;"></h3>
    <p id="tokenModalHint" class="muted"></p>
    <input id="tokenModalInput" type="password" style="width:100%;margin:8px 0;" />
    <div class="row" style="justify-content:flex-end;">
      <button id="tokenModalSave" class="active"></button>
    </div>
  </dialog>

  <dialog id="plPickerDialog">
    <h3 id="plPickerTitle" style="margin-top:0;margin-bottom:4px;"></h3>
    <div id="plPickerSub" class="muted" style="margin-bottom:8px;"></div>
    <div class="row" style="margin-bottom:10px;">
      <input id="plPickerNewName" class="grow" type="text" />
      <button id="plPickerCreateBtn" class="green"></button>
    </div>
    <div id="plPickerList" class="track-list"></div>
    <div class="row" style="margin-top:10px;justify-content:flex-end;">
      <button id="plPickerClose"></button>
    </div>
  </dialog>

  <audio id="audio" preload="metadata"></audio>

<script>
const STR = {
  es: {
    modeBrowser: "🔊 En este Navegador",
    modeRemote: "🖥 Controlar Nodo",
    tokenBtn: "🔑 Token",
    tabLib: "1. Biblioteca",
    tabSearch: "2. Buscar (FTS5)",
    tabPl: "3. Playlists & Viaje",
    tabP2p: "4. Red P2P / Nodos",
    filterPh: "Filtrar por título, artista, álbum o nodo...",
    refresh: "↻ Actualizar",
    ftsPh: "Buscar en biblioteca y nodos P2P conectados (FTS5)...",
    searchBtn: "Buscar",
    newPlPh: "Nueva playlist...",
    createPl: "+ Crear",
    playlists: "Playlists",
    playPl: "▶ Reproducir",
    renamePl: "✎ Renombrar",
    deletePl: "🗑 Borrar",
    renamePlPh: "Nuevo nombre de playlist...",
    saveName: "Guardar",
    cancel: "Cancelar",
    confirmDeletePl: "¿Borrar esta playlist?",
    confirmDeleteYes: "Sí, borrar",
    p2pNode: "Token / URI de este nodo",
    copyTicket: "📋 Copiar Token",
    discover: "📡 Buscar en LAN",
    connectPh: "Pegar rap2p://... o IP:Puerto",
    connect: "Conectar",
    peers: "Nodos Emparejados",
    noPeers: "Sin nodos remotos emparejados todavía.",
    tokenTitle: "Autenticación del Nodo P2P",
    tokenHint: "El modo LAN público está desactivado o falta el token. Pegá el token del nodo:",
    save: "Conectar",
    addPlTitle: "Agregar a Playlist",
    plPickerNewPh: "Crear nueva playlist y agregar...",
    plPickerCreateBtn: "+ Crear y agregar",
    selectedOne: "1 pista",
    selectedMany: "{n} pistas seleccionadas",
    selectVisible: "☑ Sel. visibles",
    batchEnqueue: "+ Agregar a Fila",
    enqueueRow: "+Fila",
    queueLabel: "Fila",
    batchAddPl: "+ Agregar a Playlist",
    clearSel: "✖ Limpiar",
    close: "Cerrar",
    autoOn: "Auto: ON",
    autoOff: "Auto: OFF",
    copied: "Token copiado al portapapeles"
  },
  en: {
    modeBrowser: "🔊 This Browser",
    modeRemote: "🖥 Control Node",
    tokenBtn: "🔑 Token",
    tabLib: "1. Library",
    tabSearch: "2. Search (FTS5)",
    tabPl: "3. Playlists & Travel",
    tabP2p: "4. P2P Net / Nodes",
    filterPh: "Filter by title, artist, album, or node...",
    refresh: "↻ Refresh",
    ftsPh: "Search library and connected P2P nodes (FTS5)...",
    searchBtn: "Search",
    newPlPh: "New playlist...",
    createPl: "+ Create",
    playlists: "Playlists",
    playPl: "▶ Play list",
    renamePl: "✎ Rename",
    deletePl: "🗑 Delete",
    renamePlPh: "New playlist name...",
    saveName: "Save",
    cancel: "Cancel",
    confirmDeletePl: "Delete this playlist?",
    confirmDeleteYes: "Yes, delete",
    p2pNode: "Node Pairing Token / URI",
    copyTicket: "📋 Copy Token",
    discover: "📡 Scan LAN",
    connectPh: "Paste rap2p://... or IP:Port",
    connect: "Connect",
    peers: "Paired Nodes",
    noPeers: "No paired remote peers yet.",
    tokenTitle: "P2P Node Authentication",
    tokenHint: "Open LAN sharing is disabled or token is missing. Paste node token:",
    save: "Connect",
    addPlTitle: "Add to Playlist",
    plPickerNewPh: "New playlist name...",
    plPickerCreateBtn: "+ Create & add",
    selectedOne: "1 track",
    selectedMany: "{n} tracks selected",
    selectVisible: "☑ Select visible",
    batchEnqueue: "+ Add to Queue",
    enqueueRow: "+Q",
    queueLabel: "Queue",
    batchAddPl: "+ Add to Playlist",
    clearSel: "✖ Clear",
    close: "Close",
    autoOn: "Auto: ON",
    autoOff: "Auto: OFF",
    copied: "Token copied to clipboard"
  }
};

const st = {
  lang: localStorage.getItem("rap2p_lang") || (navigator.language && navigator.language.toLowerCase().startsWith("en") ? "en" : "es"),
  token: localStorage.getItem("rap2p_token") || "",
  mode: localStorage.getItem("rap2p_mode") || "browser",
  tab: "library",
  tracks: [],
  trackMap: new Map(),
  visibleLibTracks: [],
  searchTracks: [],
  playlists: [],
  selectedPlId: null,
  selectedIds: new Set(),
  peers: [],
  peerSig: "",
  queue: [],
  queueIdx: -1,
  autoNext: true,
  remotePlayer: null,
  pendingPickerTrackIds: [],
  ticketUri: ""
};

const audio = document.getElementById("audio");

function t(k) { return (STR[st.lang] || STR.es)[k] || k; }
function fmtTime(s) {
  if (!Number.isFinite(s) || s < 0) return "00:00";
  const m = Math.floor(s / 60), sec = Math.floor(s % 60);
  return String(m).padStart(2, "0") + ":" + String(sec).padStart(2, "0");
}

function applyLang() {
  document.documentElement.lang = st.lang;
  document.getElementById("modeBrowserBtn").textContent = t("modeBrowser");
  document.getElementById("modeRemoteBtn").textContent = t("modeRemote");
  document.getElementById("tokenBtn").textContent = t("tokenBtn");
  document.getElementById("tabLibraryBtn").textContent = t("tabLib");
  document.getElementById("tabSearchBtn").textContent = t("tabSearch");
  document.getElementById("tabPlaylistsBtn").textContent = t("tabPl");
  document.getElementById("tabP2pBtn").textContent = t("tabP2p");
  document.getElementById("libFilter").placeholder = t("filterPh");
  document.getElementById("refreshBtn").textContent = t("refresh");
  document.getElementById("ftsInput").placeholder = t("ftsPh");
  document.getElementById("ftsBtn").textContent = t("searchBtn");
  document.getElementById("newPlInput").placeholder = t("newPlPh");
  document.getElementById("createPlBtn").textContent = t("createPl");
  document.getElementById("plHeader").textContent = t("playlists");
  document.getElementById("playPlBtn").textContent = t("playPl");
  document.getElementById("renamePlBtn").textContent = t("renamePl");
  document.getElementById("deletePlBtn").textContent = t("deletePl");
  document.getElementById("renamePlInput").placeholder = t("renamePlPh");
  document.getElementById("saveRenamePlBtn").textContent = t("saveName");
  document.getElementById("cancelRenamePlBtn").textContent = t("cancel");
  document.getElementById("deletePlMsg").textContent = t("confirmDeletePl");
  document.getElementById("confirmDeletePlBtn").textContent = t("confirmDeleteYes");
  document.getElementById("cancelDeletePlBtn").textContent = t("cancel");
  document.getElementById("selectVisibleBtn").textContent = t("selectVisible");
  document.getElementById("batchEnqueueBtn").textContent = t("batchEnqueue");
  document.getElementById("batchAddPlBtn").textContent = t("batchAddPl");
  document.getElementById("clearSelBtn").textContent = t("clearSel");
  document.getElementById("p2pNodeTitle").textContent = t("p2pNode");
  document.getElementById("copyTicketBtn").textContent = t("copyTicket");
  document.getElementById("discoverBtn").textContent = t("discover");
  document.getElementById("connectPeerInput").placeholder = t("connectPh");
  document.getElementById("connectPeerBtn").textContent = t("connect");
  document.getElementById("peersHeader").textContent = t("peers");
  document.getElementById("tokenModalTitle").textContent = t("tokenTitle");
  document.getElementById("tokenModalHint").textContent = t("tokenHint");
  document.getElementById("tokenModalSave").textContent = t("save");
  document.getElementById("plPickerTitle").textContent = t("addPlTitle");
  document.getElementById("plPickerNewName").placeholder = t("plPickerNewPh");
  document.getElementById("plPickerCreateBtn").textContent = t("plPickerCreateBtn");
  document.getElementById("plPickerClose").textContent = t("close");
  document.getElementById("modeBrowserBtn").classList.toggle("active", st.mode === "browser");
  document.getElementById("modeRemoteBtn").classList.toggle("active", st.mode === "remote");
  updateBatchBar();
}

function updateBatchBar() {
  const bar = document.getElementById("batchBar");
  const n = st.selectedIds.size;
  bar.classList.toggle("hidden", n === 0);
  if (n > 0) {
    document.getElementById("batchCount").textContent = t("selectedMany").replace("{n}", String(n));
  }
}

function currentTabTracks() {
  if (st.tab === "library") return st.visibleLibTracks;
  if (st.tab === "search") return st.searchTracks;
  if (st.tab === "playlists") {
    const activePl = st.playlists.find(p => p.id === st.selectedPlId);
    return activePl ? activePl.track_ids.map(id => st.trackMap.get(id)).filter(Boolean) : [];
  }
  return [];
}

function extractUrlToken() {
  const sp = new URLSearchParams(window.location.search);
  let tok = sp.get("token");
  if (!tok && window.location.hash.includes("token=")) {
    tok = new URLSearchParams(window.location.hash.slice(1)).get("token");
  }
  if (tok) {
    st.token = tok.trim();
    localStorage.setItem("rap2p_token", st.token);
  }
}

async function apiFetch(path, opts = {}) {
  const headers = Object.assign({}, opts.headers || {});
  if (st.token) headers["X-Peer-Token"] = st.token;
  const res = await fetch(path, Object.assign({}, opts, { headers }));
  if (res.status === 403) {
    document.getElementById("tokenDialog").showModal();
    throw new Error("Unauthorized (403)");
  }
  if (!res.ok) throw new Error("HTTP " + res.status);
  return res.json();
}

async function ensureAuth() {
  extractUrlToken();
  if (st.token) return true;
  try {
    const res = await fetch("/api/v1/lan-ticket");
    if (res.ok) {
      const ticket = await res.json();
      if (ticket.auth_token) {
        st.token = ticket.auth_token;
        localStorage.setItem("rap2p_token", st.token);
        return true;
      }
    }
  } catch (_) {}
  document.getElementById("tokenDialog").showModal();
  return false;
}

async function sendControl(payload) {
  await apiFetch("/api/v1/control", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload)
  });
  setTimeout(loadState, 150);
}

function renderTrackList(containerId, list, opts = {}) {
  const root = document.getElementById(containerId);
  root.replaceChildren();
  const activeId = activeCurrentTrack() ? activeCurrentTrack().id : null;
  for (let i = 0; i < list.length; i++) {
    const tr = list[i];
    const row = document.createElement("div");
    row.className = "track-item" + (activeId === tr.id ? " playing" : "");
    row.onclick = () => playListAt(list, i);

    const chk = document.createElement("input");
    chk.type = "checkbox";
    chk.className = "track-chk";
    chk.checked = st.selectedIds.has(tr.id);
    chk.onclick = (ev) => {
      ev.stopPropagation();
      if (chk.checked) st.selectedIds.add(tr.id);
      else st.selectedIds.delete(tr.id);
      updateBatchBar();
    };

    const num = document.createElement("span");
    num.className = "muted";
    num.textContent = String(i + 1);

    const meta = document.createElement("div");
    meta.className = "track-meta";
    const titleEl = document.createElement("div");
    titleEl.className = "track-title";
    titleEl.textContent = tr.title || "Untitled";
    const subEl = document.createElement("div");
    subEl.className = "track-sub";
    subEl.textContent = (tr.artist || "?") + " — " + (tr.album || "?");
    meta.append(titleEl, subEl);

    const actions = document.createElement("div");
    actions.className = "track-actions";
    if (tr.peer_node) {
      const peerBadge = document.createElement("span");
      peerBadge.className = "badge peer";
      peerBadge.textContent = "🌐 " + tr.peer_node;
      actions.appendChild(peerBadge);
    }
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = tr.format || "audio";

    const pinBtn = document.createElement("button");
    pinBtn.className = "pin-btn" + (tr.pinned ? " pinned" : "");
    pinBtn.textContent = tr.pinned ? "★" : "☆";
    pinBtn.onclick = async (ev) => {
      ev.stopPropagation();
      await sendControl({ action: "toggle_pin", track_id: tr.id });
      tr.pinned = !tr.pinned;
      await loadCatalog();
    };

    const queueBtn = document.createElement("button");
    queueBtn.textContent = t("enqueueRow");
    queueBtn.onclick = (ev) => {
      ev.stopPropagation();
      enqueueTracks([tr]);
    };

    const addBtn = document.createElement("button");
    addBtn.textContent = "+PL";
    addBtn.onclick = (ev) => {
      ev.stopPropagation();
      openPlPicker([tr.id]);
    };

    actions.append(badge, pinBtn, queueBtn, addBtn);
    if (opts.playlistId != null) {
      const remBtn = document.createElement("button");
      remBtn.className = "danger";
      remBtn.textContent = "✕";
      remBtn.onclick = async (ev) => {
        ev.stopPropagation();
        await sendControl({
          action: "remove_from_playlist",
          playlist_id: opts.playlistId,
          track_id: tr.id
        });
      };
      actions.appendChild(remBtn);
    }
    row.append(chk, num, meta, actions);
    root.appendChild(row);
  }
}

function renderPlaylists() {
  const plRoot = document.getElementById("plList");
  plRoot.replaceChildren();
  if ((!st.selectedPlId || !st.playlists.some(p => p.id === st.selectedPlId)) && st.playlists.length > 0) {
    st.selectedPlId = st.playlists[0].id;
  } else if (st.playlists.length === 0) {
    st.selectedPlId = null;
  }
  for (const pl of st.playlists) {
    const item = document.createElement("div");
    item.className = "track-item" + (pl.id === st.selectedPlId ? " playing" : "");
    item.style.gridTemplateColumns = "1fr auto";
    item.onclick = () => {
      st.selectedPlId = pl.id;
      document.getElementById("renamePlBar").classList.add("hidden");
      document.getElementById("deletePlBar").classList.add("hidden");
      renderPlaylists();
    };
    const nameEl = document.createElement("div");
    nameEl.className = "track-title";
    nameEl.textContent = (pl.pinned ? "★ " : "") + pl.name;
    const cnt = document.createElement("span");
    cnt.className = "badge";
    cnt.textContent = String(pl.track_ids.length);
    item.append(nameEl, cnt);
    plRoot.appendChild(item);
  }
  const activePl = st.playlists.find(p => p.id === st.selectedPlId);
  document.getElementById("plTracksHeader").textContent = activePl ? activePl.name : "—";
  document.getElementById("playPlBtn").disabled = !activePl;
  document.getElementById("renamePlBtn").disabled = !activePl;
  document.getElementById("deletePlBtn").disabled = !activePl;
  const tracks = activePl ? activePl.track_ids.map(id => st.trackMap.get(id)).filter(Boolean) : [];
  renderTrackList("plTrackList", tracks, { playlistId: activePl ? activePl.id : null });
}

function renderPeers() {
  document.getElementById("ticketUriText").textContent = st.ticketUri;
  const root = document.getElementById("peersList");
  root.replaceChildren();
  if (st.peers.length === 0) {
    const empty = document.createElement("div");
    empty.className = "muted";
    empty.textContent = t("noPeers");
    root.appendChild(empty);
    return;
  }
  for (const p of st.peers) {
    const row = document.createElement("div");
    row.className = "track-item";
    row.style.gridTemplateColumns = "1fr auto";
    const info = document.createElement("div");
    const n = document.createElement("div");
    n.className = "track-title";
    n.textContent = p.node_name + " (" + p.endpoint + ")";
    const s = document.createElement("div");
    s.className = "track-sub";
    s.textContent = p.track_count + " tracks · " + p.playlist_count + " playlists";
    info.append(n, s);
    const syncBtn = document.createElement("button");
    syncBtn.textContent = "↻";
    syncBtn.onclick = () => sendControl({ action: "connect_peer", target: p.endpoint });
    row.append(info, syncBtn);
    root.appendChild(row);
  }
}

function openPlPicker(trackIds) {
  const ids = Array.isArray(trackIds) ? trackIds : [trackIds];
  if (ids.length === 0) return;
  st.pendingPickerTrackIds = ids;
  const subEl = document.getElementById("plPickerSub");
  if (ids.length === 1) {
    const tr = st.trackMap.get(ids[0]);
    subEl.textContent = tr ? (tr.title + " — " + tr.artist) : t("selectedOne");
  } else {
    subEl.textContent = t("selectedMany").replace("{n}", String(ids.length));
  }
  document.getElementById("plPickerNewName").value = "";
  const listEl = document.getElementById("plPickerList");
  listEl.replaceChildren();
  for (const pl of st.playlists) {
    const btn = document.createElement("button");
    btn.textContent = pl.name + " (" + pl.track_ids.length + ")";
    btn.onclick = async () => {
      if (ids.length === 1) {
        await sendControl({ action: "add_to_playlist", playlist_id: pl.id, track_id: ids[0] });
      } else {
        await sendControl({ action: "add_tracks_to_playlist", playlist_id: pl.id, track_ids: ids });
        st.selectedIds.clear();
        updateBatchBar();
      }
      document.getElementById("plPickerDialog").close();
    };
    listEl.appendChild(btn);
  }
  document.getElementById("plPickerDialog").showModal();
}

function activeCurrentTrack() {
  if (st.mode === "remote" && st.remotePlayer && st.remotePlayer.current_track) {
    return st.remotePlayer.current_track;
  }
  return (st.queueIdx >= 0 && st.queueIdx < st.queue.length) ? st.queue[st.queueIdx] : null;
}

async function playListAt(list, idx) {
  if (!list || idx < 0 || idx >= list.length) return;
  const tr = list[idx];
  if (st.mode === "remote") {
    await sendControl({ action: "play_track", track_id: tr.id });
    return;
  }
  st.queue = list.slice();
  st.queueIdx = idx;
  startBrowserTrack(tr);
}

function startBrowserTrack(tr) {
  const qTok = encodeURIComponent(st.token);
  audio.src = "/api/v1/stream/" + tr.id + "?token=" + qTok;
  audio.play().catch(() => {});
  updateMediaSession(tr);
  updatePlayerUi();
  refreshFilteredLibrary();
}

function updateMediaSession(tr) {
  if (!("mediaSession" in navigator) || !tr) return;
  const coverUrl = "/api/v1/cover/" + tr.id + "?token=" + encodeURIComponent(st.token);
  navigator.mediaSession.metadata = new MediaMetadata({
    title: tr.title,
    artist: tr.artist,
    album: tr.album,
    artwork: [{ src: coverUrl }]
  });
  navigator.mediaSession.setActionHandler("play", () => audio.play());
  navigator.mediaSession.setActionHandler("pause", () => audio.pause());
  navigator.mediaSession.setActionHandler("previoustrack", () => playPrev());
  navigator.mediaSession.setActionHandler("nexttrack", () => playNext());
  navigator.mediaSession.setActionHandler("seekto", (d) => { if (d.seekTime != null) audio.currentTime = d.seekTime; });
}

function updatePlayerUi() {
  const tr = activeCurrentTrack();
  document.getElementById("nowTitle").textContent = tr ? tr.title : "—";
  let sub = tr ? (tr.artist + " — " + tr.album) : "rust-audio-p2p";
  if (st.mode === "browser" && st.queue.length > 0 && st.queueIdx >= 0) {
    sub += " · " + t("queueLabel") + " " + (st.queueIdx + 1) + "/" + st.queue.length;
  }
  document.getElementById("nowSub").textContent = sub;
  const coverEl = document.getElementById("coverImg");
  if (tr) {
    coverEl.src = "/api/v1/cover/" + tr.id + "?token=" + encodeURIComponent(st.token);
  } else {
    coverEl.src = "/icon.svg";
  }

  if (st.mode === "remote" && st.remotePlayer) {
    const p = st.remotePlayer;
    document.getElementById("playPauseBtn").textContent = p.state === "playing" ? "⏸" : "▶";
    document.getElementById("autoNextBtn").textContent = p.auto_next ? t("autoOn") : t("autoOff");
    const dur = p.duration_secs || 0;
    document.getElementById("curTime").textContent = fmtTime(p.position_secs);
    document.getElementById("durTime").textContent = fmtTime(dur);
    const slider = document.getElementById("seekSlider");
    slider.max = dur > 0 ? dur : 100;
    slider.value = p.position_secs || 0;
    document.getElementById("volSlider").value = Math.min(1, p.volume ?? 1);
  } else {
    document.getElementById("playPauseBtn").textContent = audio.paused ? "▶" : "⏸";
    document.getElementById("autoNextBtn").textContent = st.autoNext ? t("autoOn") : t("autoOff");
    const dur = audio.duration || (tr && tr.duration_secs) || 0;
    document.getElementById("curTime").textContent = fmtTime(audio.currentTime);
    document.getElementById("durTime").textContent = fmtTime(dur);
    const slider = document.getElementById("seekSlider");
    slider.max = dur > 0 ? dur : 100;
    slider.value = audio.currentTime || 0;
  }
}

async function playNext() {
  if (st.mode === "remote") return sendControl({ action: "next" });
  if (st.queueIdx + 1 < st.queue.length) {
    st.queueIdx += 1;
    startBrowserTrack(st.queue[st.queueIdx]);
  }
}

async function playPrev() {
  if (st.mode === "remote") return sendControl({ action: "prev" });
  if (st.queueIdx > 0) {
    st.queueIdx -= 1;
    startBrowserTrack(st.queue[st.queueIdx]);
  } else if (st.queueIdx === 0) {
    audio.currentTime = 0;
  }
}

let libSearchTimer = null;

function refreshFilteredLibrary() {
  const q = document.getElementById("libFilter").value.trim().toLowerCase();
  const filtered = q
    ? st.tracks.filter(tr =>
        (tr.title || "").toLowerCase().includes(q) ||
        (tr.artist || "").toLowerCase().includes(q) ||
        (tr.album || "").toLowerCase().includes(q) ||
        (tr.format || "").toLowerCase().includes(q) ||
        (tr.peer_node || "").toLowerCase().includes(q))
    : st.tracks;
  st.visibleLibTracks = filtered;
  renderTrackList("libList", filtered);

  if (q.length >= 2) {
    clearTimeout(libSearchTimer);
    libSearchTimer = setTimeout(async () => {
      try {
        const remoteHits = await apiFetch("/api/v1/search?q=" + encodeURIComponent(q));
        if (Array.isArray(remoteHits) && remoteHits.length > 0) {
          let changed = false;
          for (const h of remoteHits) {
            if (!st.trackMap.has(h.id)) {
              st.tracks.push(h);
              st.trackMap.set(h.id, h);
              changed = true;
            }
          }
          if (changed) refreshFilteredLibrary();
        }
      } catch (_) {}
    }, 180);
  }
}

async function loadCatalog() {
  const cat = await apiFetch("/api/v1/catalog?include_peer=1");
  st.tracks = cat.tracks || [];
  st.trackMap = new Map(st.tracks.map(x => [x.id, x]));
  refreshFilteredLibrary();
  renderPlaylists();
}

async function loadState() {
  try {
    const s = await apiFetch("/api/v1/state");
    document.getElementById("nodeBadge").textContent = s.node_name + " · " + s.endpoint;
    st.ticketUri = s.ticket_uri;
    st.playlists = s.playlists || [];
    st.peers = s.peers || [];
    const nextPeerSig = JSON.stringify(st.peers);
    if (st.peerSig && st.peerSig !== nextPeerSig) {
      loadCatalog().catch(() => {});
    }
    st.peerSig = nextPeerSig;
    st.remotePlayer = s.player || null;
    renderPlaylists();
    renderPeers();
    updatePlayerUi();
  } catch (_) {}
}

async function runFtsSearch() {
  const q = document.getElementById("ftsInput").value.trim();
  const items = await apiFetch("/api/v1/search?q=" + encodeURIComponent(q));
  st.searchTracks = items || [];
  renderTrackList("searchList", st.searchTracks);
}

document.querySelectorAll(".tab-btn").forEach(btn => {
  btn.onclick = () => {
    st.tab = btn.dataset.tab;
    document.querySelectorAll(".tab-btn").forEach(b => b.classList.toggle("active", b === btn));
    document.querySelectorAll(".view").forEach(v => v.classList.toggle("hidden", v.id !== "view-" + st.tab));
  };
});

document.getElementById("modeBrowserBtn").onclick = () => {
  st.mode = "browser"; localStorage.setItem("rap2p_mode", st.mode); applyLang(); updatePlayerUi();
};
document.getElementById("modeRemoteBtn").onclick = () => {
  st.mode = "remote"; localStorage.setItem("rap2p_mode", st.mode); applyLang(); updatePlayerUi();
};
document.getElementById("langBtn").onclick = () => {
  st.lang = st.lang === "es" ? "en" : "es"; localStorage.setItem("rap2p_lang", st.lang); applyLang();
};
document.getElementById("tokenBtn").onclick = () => document.getElementById("tokenDialog").showModal();
document.getElementById("tokenModalSave").onclick = async () => {
  const raw = document.getElementById("tokenModalInput").value.trim();
  const hashTok = raw.includes("#") ? raw.split("#")[1].split("?")[0].trim() : raw;
  st.token = hashTok;
  localStorage.setItem("rap2p_token", st.token);
  document.getElementById("tokenDialog").close();
  await loadCatalog();
  await loadState();
};

document.getElementById("libFilter").oninput = refreshFilteredLibrary;
document.getElementById("refreshBtn").onclick = async () => { await loadCatalog(); await loadState(); };
document.getElementById("ftsInput").oninput = () => runFtsSearch().catch(() => {});
document.getElementById("ftsBtn").onclick = () => runFtsSearch().catch(() => {});

document.getElementById("createPlBtn").onclick = async () => {
  const name = document.getElementById("newPlInput").value.trim();
  if (!name) return;
  document.getElementById("newPlInput").value = "";
  await sendControl({ action: "create_playlist", name, track_id: null });
};
document.getElementById("playPlBtn").onclick = () => {
  const pl = st.playlists.find(p => p.id === st.selectedPlId);
  if (!pl) return;
  const tracks = pl.track_ids.map(id => st.trackMap.get(id)).filter(Boolean);
  playListAt(tracks, 0);
};
document.getElementById("renamePlBtn").onclick = () => {
  const pl = st.playlists.find(p => p.id === st.selectedPlId);
  if (!pl) return;
  document.getElementById("deletePlBar").classList.add("hidden");
  document.getElementById("renamePlInput").value = pl.name;
  document.getElementById("renamePlBar").classList.remove("hidden");
  document.getElementById("renamePlInput").focus();
};
document.getElementById("saveRenamePlBtn").onclick = async () => {
  const pl = st.playlists.find(p => p.id === st.selectedPlId);
  const newName = document.getElementById("renamePlInput").value.trim();
  if (!pl || !newName) return;
  document.getElementById("renamePlBar").classList.add("hidden");
  await sendControl({ action: "rename_playlist", playlist_id: pl.id, new_name: newName });
};
document.getElementById("cancelRenamePlBtn").onclick = () => {
  document.getElementById("renamePlBar").classList.add("hidden");
};
document.getElementById("deletePlBtn").onclick = () => {
  const pl = st.playlists.find(p => p.id === st.selectedPlId);
  if (!pl) return;
  document.getElementById("renamePlBar").classList.add("hidden");
  document.getElementById("deletePlBar").classList.remove("hidden");
};
document.getElementById("confirmDeletePlBtn").onclick = async () => {
  const pl = st.playlists.find(p => p.id === st.selectedPlId);
  document.getElementById("deletePlBar").classList.add("hidden");
  if (!pl) return;
  st.selectedPlId = null;
  await sendControl({ action: "delete_playlist", playlist_id: pl.id });
};
document.getElementById("cancelDeletePlBtn").onclick = () => {
  document.getElementById("deletePlBar").classList.add("hidden");
};
document.getElementById("selectVisibleBtn").onclick = () => {
  for (const tr of currentTabTracks()) st.selectedIds.add(tr.id);
  updateBatchBar();
  refreshFilteredLibrary();
  if (st.searchTracks.length) renderTrackList("searchList", st.searchTracks);
  renderPlaylists();
};
document.getElementById("batchEnqueueBtn").onclick = () => {
  if (st.selectedIds.size === 0) return;
  const list = Array.from(st.selectedIds).map(id => st.trackMap.get(id)).filter(Boolean);
  enqueueTracks(list);
  st.selectedIds.clear();
  updateBatchBar();
  refreshFilteredLibrary();
  if (st.searchTracks.length) renderTrackList("searchList", st.searchTracks);
  renderPlaylists();
};
document.getElementById("batchAddPlBtn").onclick = () => {
  if (st.selectedIds.size > 0) openPlPicker(Array.from(st.selectedIds));
};

async function enqueueTracks(list) {
  if (!list || list.length === 0) return;
  if (st.mode === "remote") {
    if (list.length === 1) {
      await sendControl({ action: "enqueue_track", track_id: list[0].id });
    } else {
      await sendControl({ action: "enqueue_tracks", track_ids: list.map(x => x.id) });
    }
    return;
  }
  const startIdx = st.queue.length;
  for (const item of list) st.queue.push(item);
  if (st.queueIdx < 0 || !audio.src || (audio.paused && audio.ended)) {
    st.queueIdx = startIdx;
    startBrowserTrack(st.queue[st.queueIdx]);
  } else {
    updatePlayerUi();
  }
}
document.getElementById("clearSelBtn").onclick = () => {
  st.selectedIds.clear();
  updateBatchBar();
  refreshFilteredLibrary();
  if (st.searchTracks.length) renderTrackList("searchList", st.searchTracks);
  renderPlaylists();
};
document.getElementById("plPickerCreateBtn").onclick = async () => {
  const name = document.getElementById("plPickerNewName").value.trim();
  const ids = st.pendingPickerTrackIds || [];
  if (!name || ids.length === 0) return;
  if (ids.length === 1) {
    await sendControl({ action: "create_playlist", name, track_id: ids[0] });
  } else {
    await sendControl({ action: "create_playlist_with_tracks", name, track_ids: ids });
    st.selectedIds.clear();
    updateBatchBar();
  }
  document.getElementById("plPickerDialog").close();
};
document.getElementById("plPickerClose").onclick = () => document.getElementById("plPickerDialog").close();

document.getElementById("copyTicketBtn").onclick = async () => {
  if (navigator.clipboard && st.ticketUri) {
    await navigator.clipboard.writeText(st.ticketUri);
    alert(t("copied"));
  }
};
document.getElementById("discoverBtn").onclick = () => sendControl({ action: "discover_lan" });
document.getElementById("connectPeerBtn").onclick = async () => {
  const target = document.getElementById("connectPeerInput").value.trim();
  if (!target) return;
  document.getElementById("connectPeerInput").value = "";
  await sendControl({ action: "connect_peer", target });
};

document.getElementById("playPauseBtn").onclick = () => {
  if (st.mode === "remote") return sendControl({ action: "toggle_pause" });
  if (audio.paused) audio.play().catch(() => {}); else audio.pause();
  updatePlayerUi();
};
document.getElementById("nextBtn").onclick = () => playNext();
document.getElementById("prevBtn").onclick = () => playPrev();
document.getElementById("autoNextBtn").onclick = () => {
  if (st.mode === "remote") return sendControl({ action: "toggle_auto_next" });
  st.autoNext = !st.autoNext;
  updatePlayerUi();
};
document.getElementById("seekSlider").onchange = (e) => {
  const pos = parseFloat(e.target.value);
  if (st.mode === "remote") return sendControl({ action: "seek", position_secs: pos });
  if (Number.isFinite(pos)) audio.currentTime = pos;
};
document.getElementById("volSlider").oninput = (e) => {
  const vol = parseFloat(e.target.value);
  if (st.mode === "remote") return sendControl({ action: "set_volume", volume: vol });
  audio.volume = Math.max(0, Math.min(1, vol));
};

audio.addEventListener("timeupdate", updatePlayerUi);
audio.addEventListener("loadedmetadata", updatePlayerUi);
audio.addEventListener("play", updatePlayerUi);
audio.addEventListener("pause", updatePlayerUi);
audio.addEventListener("ended", () => { if (st.autoNext && st.mode === "browser") playNext(); });

(async () => {
  applyLang();
  if (await ensureAuth()) {
    await loadCatalog();
    await loadState();
  }
  setInterval(loadState, 2000);
})();
</script>
</body>
</html>
"##;
