# rust-audio-p2p

Plataforma de audio distribuida en **Rust** que unifica **biblioteca local (`$HOME` / `%USERPROFILE%`)**, **descarga y streaming BitTorrent (`librqbit`)**, **caché LRU + pines offline para viajes**, **red P2P liviana entre dispositivos (`rap2p://` + QR)**, **Web UI / PWA**, **GUI nativa de escritorio** y **soporte móvil Android / Kotlin + Windows + macOS**.

Distributed audio platform in **Rust** unifying **local audio libraries (`$HOME` / `%USERPROFILE%`)**, **BitTorrent streaming/downloading (`librqbit`)**, **LRU cache + offline travel pins**, **lightweight serverless P2P audio sharing (`rap2p://` + QR)**, **Web UI / PWA**, **native desktop GUI**, and **Android / Windows / macOS support**.

---

## Instalación por Plataforma / Installation

### 1. Linux (Ubuntu / Kubuntu / Debian) — Paquete `.deb` con repo APT firmado
Descargá el último `.deb` desde [GitHub Releases](https://github.com/niunmango/rust-audio-p2p/releases/latest) (o desde [GitHub Pages](https://niunmango.github.io/rust-audio-p2p/)):
```bash
sudo apt install ./rust-audio-p2p_latest_amd64.deb
```
Instala tanto la **GUI de escritorio** (`rust-audio-gui` / lanzador en el menú de aplicaciones) como la **TUI en terminal** (`rust-audio-tui`), y configura `/etc/apt/sources.list.d/rust-audio-p2p.list` con firma GPG. Para actualizar en el futuro:
```bash
sudo apt update && sudo apt upgrade
```

### 2. Portable `.AppImage` (cualquier distribución Linux `x86_64`)
```bash
chmod +x rust-audio-p2p-latest-x86_64.AppImage
./rust-audio-p2p-latest-x86_64.AppImage
```

### 3. Windows y macOS / Build desde código fuente
- **Requisitos Linux:** `sudo apt-get install -y pkg-config libasound2-dev libxkbcommon0 libxkbcommon-x11-0 libgl1 fakeroot`
- **Windows (`x86_64-pc-windows-msvc`) / macOS (`aarch64-apple-darwin`):** Rust stable (`rustup`), ejecuta auto-escaneo sobre `%USERPROFILE%` / `$HOME` excluyendo `AppData`, `Library`, `Applications`, `$RECYCLE.BIN`.
```bash
cargo test --workspace
cargo build --release -p rust-audio-tui -p rust-audio-gui
```
*(Opcional en GitHub Actions: workflows manuales [android.yml](.github/workflows/android.yml) y [cross-platform.yml](.github/workflows/cross-platform.yml) vía `workflow_dispatch`).*

### 4. Web UI / PWA (Navegador y Celular en Red Local)
Al abrir la TUI o la GUI en tu PC o servidor headless, la barra lateral / pestaña `6: Red P2P / Nodos` muestra tu enlace local:
`http://<IP_LAN>:4261` (o `:4262`).
Abrilo en Chrome/Firefox/Android para navegar biblioteca local + nodos P2P conectados, encolar temas (`+Fila`), crear/renombrar/borrar playlists o usar el navegador como control remoto.

### 5. Android (Kotlin + Jetpack Compose + Media3 + `crates/android-ffi`)
El módulo [crates/android-ffi](crates/android-ffi/src/lib.rs) expone una sesión portable (`AndroidCoreSession`, puente C-ABI `rap2p_android_invoke_json`) con SQLite FTS5, playlists, cola, pines offline y políticas de ahorro (`Solo Wi-Fi` para descargas/sincronización y `Solo cargando` para seeding), integrado con el proyecto Kotlin/Compose en [android/](android/app/src/main/java/io/github/niunmango/rustaudiop2p/MainActivity.kt).

---

## Atajos y Controles Rápidos / Shortcuts (`?` o `F1` para ver en app)

- **Reproducción:** `Space` Pausar/Reanudar · `Enter` / Doble clic Reproducir · `← / →` Seek ±10s · `+ / -` Volumen · `n / b` Siguiente/Anterior · `A` Auto-Siguiente
- **Fila de reproducción y Playlists:**
  - `w` o botón **`+Fila` (`+Queue`)**: agrega una o varias pistas marcadas a la fila de reproducción
  - `x` / Casilla: marcar/desmarcar múltiples pistas · `X` limpiar selección
  - `a` / `+PL`: agregar pista(s) a una playlist existente o crear una nueva
  - `p` / `★`: alternar Pin Offline (Modo Viaje)
  - En `3: Playlists`: `n` nueva · `e` renombrar · `d` / `Del` borrar playlist o quitar pista
- **Biblioteca / Búsqueda / P2P:** `f` filtro rápido en Biblioteca (TUI) · `/` FTS5 · `r` re-escanear · `c / d / s / v` conectar/LAN/QR en P2P · `l` idioma (`Auto` / `ES` / `EN`) · `?` o `F1` Ayuda

---

## Arquitectura del Workspace / Workspace Architecture

- `crates/core` (`rust-audio-core`): Modelos, SQLite (`bundled`) + FTS5, i18n (`es`/`en`), caché LRU, pines offline, cola (`PlaybackQueue`) y prefetch.
- `crates/source-local` (`rust-audio-source-local`): Auto-escaneo incremental cross-platform (`$HOME` / `%USERPROFILE%`), lectura de tags/portadas (`lofty`), `notify` y lectura con *seek*.
- `crates/source-torrent` (`rust-audio-source-torrent`): Motor BitTorrent (`librqbit` con `rustls` en puertos `4240–4260`) y adaptadores Torznab/Jackett + Internet Archive.
- `crates/net-friends` (`rust-audio-net-friends`): Red P2P LAN/friend-to-friend (`rap2p://` + QR), gateway HTTP con streaming bajo demanda de nodos conectados y Web UI / PWA.
- `crates/player` (`rust-audio-player`): Motor (`rodio`/`symphonia`) con PipeWire/PulseAudio/ALSA y fallback headless `NullSink`.
- `crates/android-ffi` (`rust-audio-android-ffi`): Core móvil C-ABI JSON + políticas de Wi-Fi y batería para Android (`cdylib`/`staticlib`).
- `bins/gui` (`rust-audio-gui`): GUI nativa en Desktop (`eframe`/`egui` + OpenGL `glow`) con barra lateral, tabla virtualizada `O(1)`, dock de reproducción, filtros y ventana de ayuda (`F1` / `?`).
- `bins/tui` (`rust-audio-tui`): Interfaz de terminal (`ratatui`) con mouse, *bracketed paste*, filtro rápido, cola, playlists y ayuda (`?` / `F1`).

---

## Seguridad y Privacidad / Security & Privacy

- **Anti DNS-Rebinding & CSRF (`crates/net-friends`):** Validación estricta de `Host`, `Origin` y `Sec-Fetch-Site` en el servidor HTTP/Web UI, headers CSP / `nosniff` / `X-Frame-Options: DENY` / `Referrer-Policy: no-referrer`, comparación en tiempo constante de tokens (`X-Peer-Token`), límite de conexiones concurrentes, validación SSRF de endpoints P2P y clientes HTTP sin redirecciones automáticas.
- **Permisos locales y Anti-Symlink:** En Unix, `data_dir` y `cache_dir` se restringen a `0700` y `library.db` / archivos `.part` a `0600`; el escáner y el caché rechazan symlinks y path traversal.
- **Integridad de paquetes:** Artículos `.deb` y `.AppImage` publicados con `SHA256SUMS.txt` y firma GPG en APT (`APT_GPG_PRIVATE_KEY`).

## Licencia / License
MIT — ver [LICENSE](LICENSE).
