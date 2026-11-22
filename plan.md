# Plan de Desarrollo por Fases — Reproductor y Red de Audio Distribuida en Rust (`rust-audio-p2p`)

## Contexto y Reglas de Ejecución para el Agente (`Antigravity CLI` + `Orchestrator Argon`)

Este documento define el plan de ingeniería para desarrollar de forma incremental una plataforma de audio distribuida en **Rust** que unifica **archivos en disco local**, **descarga/streaming vía BitTorrent (`librqbit`)**, **caché LRU + pines offline para viajes**, y **red privada entre amigos sin servidor central (`iroh`)**.

### Reglas Operativas Obligatorias (Aplican a TODAS las fases)
1. **Trabajo estrictamente por fases:** Ejecutá **una sola fase por ciclo**. Nunca avances automáticamente a la fase siguiente sin detenerte y pedir confirmación explícita del usuario.
2. **Git & GitHub en Fase 0 / Fase 1:**
   - En la **Fase 0**, creá un repositorio público en GitHub bajo la cuenta del usuario (`niunmango`), con **licencia MIT** (`LICENSE`), `README.md`, `.gitignore` para Rust/Cargo y estructura inicial del *workspace*.
   - Al completar **cada fase**, verificá que compile sin errores críticos (`cargo check` / `cargo test` según corresponda), realizá un **commit local** claro y descriptivo, y hacé **`git push`** al repositorio público de GitHub.
3. **Punto de control obligatorios al final de cada fase:**
   - Al terminar una fase y pushear a GitHub, presentá un resumen de lo implementado, cómo probarlo y **preguntá explícitamente al usuario si aprueba pasar a la siguiente fase** antes de tocar código de la fase posterior.
4. **Calidad y soberanía técnica:**
   - Priorizá código Rust limpio, idiomático, modular (`workspace` con *crates* desacoplados) y sin dependencias de servicios propietarios o nubes centralizadas.
   - Aislá las búsquedas en trackers/indexadores externos detrás de un *trait* configurable para no acoplar el núcleo a sitios específicos.
5. **Internacionalización (i18n) desde el núcleo:**
   - Ninguna interfaz (`tui`, `gui`, `web`, `android`) debe tener textos de usuario *hardcodeados*. Todo texto visible debe provenir del sistema i18n compartido en `crates/core` (módulo `i18n`), soportando inicialmente **Español (`es`)** e **Inglés (`en`)** con autodetección del idioma del sistema (`sys-locale` / variables `LC_ALL`, `LC_MESSAGES`, `LANG` / *locale* de navegador o Android), *fallback* configurable y selector manual persistido en configuración.

---

## Perfil de Hardware y Entorno Dual (Desarrollo en `air` vs. Ejecución en `Kubuntu 26.04`)

Este plan contempla dos máquinas diferenciadas: la máquina actual donde se desarrolla y compila (`air`), y la máquina de escritorio destino donde se usará cotidianamente (`Kubuntu 26.04`).

### 1. Máquina de Desarrollo Actual (`air` — MacBook Air 2012, Ubuntu 24.04.5 LTS Server Headless)
* **CPU y RAM:** Intel Core i5-3317U @ 1.70GHz (2 núcleos / 4 hilos) y **7.7 GiB de RAM** (~3.1 GiB consumidos por 12 contenedores Podman de `media-stack`, ~4.5 GiB disponibles + 4 GiB de swap).
* **Almacenamiento:** **~69.5 GiB libres** en `/` (`/dev/mapper/ubuntu--vg-ubuntu--lv`).
* **GPU y Display:** Intel HD Graphics 4000 (Ivy Bridge, OpenGL 4.0) en entorno Ubuntu Server **headless** (sin servidor X11 ni Wayland activo). Su rol principal es **desarrollo, compilación y pruebas**, y eventualmente ejecución en modo **TUI** reproduciendo por sus parlantes internos vía ALSA (`/dev/snd`).
* **Servicios y datos locales existentes en `air`:**
  * **Biblioteca musical existente:** `/home/ramiro/media-stack/data/media/music` (archivos FLAC estructurados como `<Artista>/<Álbum> (<Año>)/01 - <Canción>.flac` con `cover.jpg`, `folder.jpg` y `artist.jpg`).
  * **Jackett** corriendo en `http://localhost:9117` (API Torznab) y **FlareSolverr** en `http://localhost:8191`.
  * **qBittorrent** ocupando los puertos **TCP/UDP `6881`** y **UDP `6771`**.

### 2. Máquina Destino Principal (`Kubuntu 26.04`)
* PC de escritorio con KDE Plasma (Wayland/X11) y PipeWire donde correrán en producción tanto la **TUI (`bins/tui`)** como la **GUI nativa (`bins/gui`)** sobre una sesión gráfica completa.

### 3. Reglas de Compilación para Cuidar RAM y Disco en `air`
* **Paralelismo acotado:** Configurar `.cargo/config.toml` con `[build] jobs = 2` para evitar picos de memoria / *swap thrashing* y proteger los contenedores Podman activos.
* **Perfiles livianos en `Cargo.toml`:** Establecer `[profile.dev] debug = 0` y `[profile.test] debug = 0` en el `Cargo.toml` raíz para reducir ~70% el peso de `target/` y el consumo de RAM del linker.
* **Cero dependencias C innecesarias:**
  * Usar `rusqlite` con `features = ["bundled"]` (incluye SQLite + FTS5 sin requerir `libsqlite3-dev`).
  * Forzar TLS vía **`rustls`** en los crates de red (`reqwest`, `librqbit`, `iroh`) para no depender de `libssl-dev` / OpenSSL del sistema.

---

## Estructura Base del Workspace (Fase 0)

```text
rust-audio-p2p/
├── .cargo/
│   └── config.toml      # jobs = 2 para cuidar RAM en air
├── Cargo.toml           # Workspace + profile.dev / profile.test con debug = 0
├── LICENSE
├── README.md
├── .gitignore
├── crates/
│   ├── core/            # Modelos de dominio, SQLite (bundled) + FTS5, i18n (es/en autodetectado), caché LRU, pines offline, cola & prefetch
│   ├── source-local/    # Auto-escaneo de $HOME, exclusiones inteligentes, tags/carátulas (lofty), seek
│   ├── source-torrent/  # Motor BitTorrent (librqbit, puertos dedicados) + trait de buscadores (Torznab/Jackett)
│   ├── net-friends/     # Red P2P con iroh, QR/Ticket, sync de playlists e intercambio
│   └── player/          # Reproductor (rodio/symphonia + fallback NullSink headless), cola, prefetch y estado
└── bins/
    ├── tui/             # Fase 1: Interfaz en terminal (ratatui)
    └── gui/             # Fase 2: Interfaz gráfica de escritorio (Kubuntu 26.04 / OpenGL glow)
```

---

## Detalle por Fases

### Fase 0: Bootstrap del Repositorio Público (MIT) y Workspace Rust

* **Objetivo:** Preparar los prerrequisitos mínimos de sistema en `air`, crear el repositorio público en GitHub con licencia MIT y levantar el esqueleto compilable del *workspace* en Rust optimizado para los recursos de esta PC.
* **Tareas:**
1. **Prerrequisitos de sistema en `air`:** Verificar e instalar `pkg-config` y `libasound2-dev` (`sudo apt-get update && sudo apt-get install -y pkg-config libasound2-dev`, requeridos por `cpal`/`rodio` para ALSA) y asegurar que el usuario `ramiro` pertenezca al grupo `audio` (`sudo usermod -aG audio ramiro`) para acceder a `/dev/snd/*`.
2. **Autenticación con GitHub y creación del repositorio:**
   * **Nota de entorno en `air`:** `/home/ramiro/.local/bin/gh` **no** es el GitHub CLI oficial, sino un script Python (`github browser opener v0.0.4`) que falla con `gh auth status`.
   * Crear el repositorio público en GitHub (ej. `rust-audio-p2p`) con licencia **MIT** bajo la cuenta configurada (`user.name=niunmango`) usando la **API REST de GitHub (`https://api.github.com/user/repos`) vía `curl` con el token almacenado en `~/.git-credentials`** (o instalando el paquete oficial `gh` desde APT).
3. Inicializar el repositorio local, configurar el remoto `origin`, crear `.gitignore` (ignorando `target/`, bases SQLite locales de prueba, archivos de caché y `.env`), `LICENSE` (MIT) y `README.md` describiendo la visión por fases.
4. Crear `.cargo/config.toml` (`[build] jobs = 2`), el `Cargo.toml` del *workspace* (con `[profile.dev] debug = 0` y `[profile.test] debug = 0`) y los esqueletos de `crates/core`, `crates/source-local`, `crates/source-torrent`, `crates/net-friends`, `crates/player` y `bins/tui`.
5. Ejecutar `cargo check --workspace`.

* **Criterio de salida & Checkpoint:** Los prerrequisitos de compilación están listos, el repo público existe en GitHub con licencia MIT, `cargo check --workspace` pasa en verde, hay commit local y `push` realizado. **Preguntar al usuario antes de iniciar la Fase 1.**

---

### Fase 1: Linux + TUI (Auto-escaneo de `$HOME`, Índice SQLite, Reproducción con Seek, Caché LRU, Pines Offline y Torrent)

* **Objetivo:** Contar con una aplicación funcional en **Linux con TUI (`ratatui`)** que al arrancar escanee automáticamente el `$HOME` del usuario, indexe música local y mediante *torrents* (`librqbit`) en SQLite (FTS5), con búsqueda rápida, reproducción con *seek*, caché LRU, pines offline para viajes y *prefetch* del siguiente tema.
* **Tareas:**
1. **`crates/core`:**
   * Definir modelos unificados (`Track`, `Album`, `Artist`, `Playlist`, `TrackSource` con variantes `LocalFile` y `Torrent`).
   * Implementar persistencia en **SQLite** usando `rusqlite` con `features = ["bundled"]` (y **FTS5** para búsqueda por título, artista y álbum, más tablas para *playlists*, historial y estado de caché/pines).
   * Implementar el módulo **`i18n`** con catálogos completos en **Español (`es`)** e **Inglés (`en`)**, autodetección del *locale* del sistema operativo al arrancar (`sys-locale` + inspección de `LC_ALL`/`LC_MESSAGES`/`LANG`), *fallback* configurable y cambio de idioma en caliente persistido en SQLite/config.
   * Implementar **Caché LRU** con límite configurable (default **3 GB** para proteger los ~69.5 GB libres en `air`) y **Pines Offline** (protege pistas/álbumes/playlists marcadas para viaje). Los archivos `TrackSource::LocalFile` deben reproducirse *in-place* sin duplicarse en la caché LRU (o usar *hardlinks* cuando corresponda dentro del mismo filesystem).
   * Implementar gestor de cola y **prefetch** (al reproducir el tema $N$, pre-descargar/pre-bufferizar el tema $N+1$).

2. **`crates/source-local`:**
   * **Auto-escaneo del `$HOME` al arrancar:** Al iniciar la aplicación, lanzar automáticamente un escaneo asíncrono en segundo plano sobre `$HOME` (`dirs::home_dir()`) para descubrir e incorporar a la biblioteca SQLite los archivos de audio soportados (`.flac`, `.mp3`, `.ogg`, `.opus`, `.m4a`, `.wav`, `.aac`), funcionando de forma transparente tanto en **Kubuntu 26.04** como en **`air`** (donde detectará automáticamente `/home/ramiro/media-stack/data/media/music`).
   * **Filtro inteligente de exclusión en `$HOME`:** Excluir directorios ocultos (`.*`), cachés y árboles pesados de desarrollo/contenedores (`.cargo`, `.rustup`, `.cache`, `.local/share/containers`, `target`, `node_modules`, `snap`, `.git`) para evitar recorrer capas de Podman o artefactos de build.
   * Monitorear cambios incrementales (`walkdir` + `notify`), leer metadatos y carátulas embebidas (`lofty`) además de detectar archivos de portada en carpeta (`cover.jpg`, `folder.jpg`, `artist.jpg`), y soportar lectura con *seek* (*byte-range* / posiciones de bytes o tiempo).

3. **`crates/source-torrent`:**
   * Integrar `librqbit` (con TLS vía `rustls`) para gestionar *magnet links* y archivos `.torrent`, priorizando piezas iniciales para reproducción temprana, *seek* y *seeding* de lo descargado/local.
   * **Prevención de conflicto de puertos en `air`:** Configurar `librqbit` en un rango de puertos dedicado (ej. `4240-4260` o puerto dinámico) distinto de `6881` (TCP/UDP) y `6771` (UDP), ya ocupados por el contenedor `qBittorrent` en `air`.
   * Definir el *trait* `TorrentSearchProvider` e implementar un adaptador **`TorznabSearchProvider`** compatible con el servicio **Jackett** local (`http://localhost:9117` en `air`, configurable por archivo/variables de entorno), junto con un adaptador genérico/configurable para entornos sin Jackett (como una instalación limpia en Kubuntu 26.04).

4. **`crates/player`:**
   * Implementar motor de reproducción (play, pause, stop, next/prev, *seek*, volumen, estado actual) con `rodio`/`symphonia` compatible con PipeWire/PulseAudio/ALSA (tanto para **Kubuntu 26.04** como para los parlantes internos ALSA de la **MacBook Air 2012**).
   * Incluir un **modo fallback sin dispositivo de audio (`NullSink` / headless)** para que la TUI, el avance de cola, el *seek*, el *prefetch* y los tests corran sin fallar cuando se ejecute por SSH o sin salida de audio activa.

5. **`bins/tui`:**
   * Interfaz en terminal con `ratatui` que renderice todos sus textos, menús, atajos, estados de escaneo/caché y mensajes de error mediante `core::i18n` (autodetectando **Español** o **Inglés** y permitiendo alternar entre `Auto` / `es` / `en` desde la TUI), y que permita: ver el progreso del auto-escaneo de `$HOME`, navegar biblioteca local y resultados/descargas torrent, buscar (FTS5), gestionar *playlists* y pines offline ("modo viaje"), ver estado de caché/descargas y controlar la reproducción.

* **Criterio de salida & Checkpoint:** La TUI compila y corre en Linux (`air`), autodetecta Español/Inglés según el *locale* del sistema y permite alternar entre ambos idiomas, auto-escanea `$HOME`, reproduce archivos locales y *torrents* con *seek* (o valida el flujo en modo `NullSink`/ALSA), respeta caché LRU y pines offline, tiene commit local y `push` a GitHub. **Preguntar al usuario antes de iniciar la Fase 2.**

---

### Fase 2: GUI Nativa en Linux (`Kubuntu 26.04` + Build Headless en `air`)

* **Objetivo:** Agregar una interfaz gráfica de escritorio en Linux (`bins/gui`) pensada para ejecutarse en **Kubuntu 26.04** (Wayland/X11) sobre el mismo *core* de Rust, sin duplicar lógica de negocio y compilable de forma headless desde `air`.
* **Tareas:**
1. Crear `bins/gui` usando `eframe`/`egui` con soporte de renderizado **OpenGL (`glow`)**, asegurando compatibilidad universal tanto con **Kubuntu 26.04** como con GPUs veteranas (como la **Intel HD 4000** de la MacBook Air 2012, cuyo soporte Vulkan `hasvk` es incompleto), conectado directamente a `core`, `player`, `source-local` y `source-torrent`.
2. Instalar en `air` las librerías de desarrollo necesarias para compilar la GUI en entorno headless (`libxcb-render0-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libgl1-mesa-dev`).
3. Implementar vistas principales usando el mismo módulo `i18n` de `crates/core`: Biblioteca/Búsqueda (con indicador de auto-escaneo de `$HOME`), *Playlists* y Pines Offline (viajes), Descargas/Torrent, selector de idioma (`Auto (es/en)`, `Español`, `English`) y Barra inferior de reproducción con *seek* y volumen.
4. Compartir la misma base SQLite, caché y configuración (incluida la preferencia de idioma) con `bins/tui` para poder alternar entre TUI y GUI sin perder estado.

* **Criterio de salida & Checkpoint:** Como `air` es Ubuntu Server headless (sin X11/Wayland activo), el checkpoint en `air` valida que `bins/gui` compile limpiamente (`cargo check -p gui` y `cargo build -p gui`) y pase los tests de integración con el *core* compartido, quedando listo para ejecutarse gráficamente en **Kubuntu 26.04**; commit local y `push` a GitHub realizados. **Preguntar al usuario antes de iniciar la Fase 3.**

---

### Fase 3: Red Privada entre Amigos en Linux (Sin Servidor Central + QR/Token + Sync)

* **Objetivo:** Permitir que amigos conecten sus nodos de Linux **sin servidor central**, mediante un **Token / QR**, sincronizando *playlists* y compartiendo/descargando archivos locales o *torrents* de forma distribuida.
* **Tareas:**
1. **`crates/net-friends`:**
   * Integrar `iroh` (conexiones directas QUIC cifradas, *NAT traversal / hole punching* y *relays* para evitar abrir puertos o sufrir CGNAT, manteniendo `rustls`).
   * Diseñar y generar **Tickets / Tokens** (codificables en texto y **QR** en terminal/GUI) que incluyan identidad del nodo (`NodeId`), *hints* de conexión y secreto/firma de red de amigos para rechazar pares no autorizados.
   * Implementar sincronización de *playlists* y metadatos de pistas compartidas entre nodos emparejados.
   * Permitir pedir y *seedear* archivos locales o piezas entre amigos, integrándolos con la caché LRU y los pines offline.
2. Integrar el flujo de "Invitar / Unirse por QR o Token" y "Compartir *playlist* con amigos" en la TUI y la GUI de Linux, con todos los textos y estados localizados en `es` y `en` vía `core::i18n`.
3. Probar la conexión y transferencia entre dos instancias/nodos.

* **Criterio de salida & Checkpoint:** Dos nodos en Linux pueden emparejarse por Token/QR sin servidor central, sincronizar *playlists* y transferir/compartir audio; commit local y `push` a GitHub realizados. **Preguntar al usuario antes de iniciar la Fase 4.**

---

### Fase 4: App para Android (`Core` Rust Local + Build de APK en GitHub Actions / Web)

* **Objetivo:** Llevar el *core* en Rust y la red/caché/pines offline a **Android**, con reproducción en segundo plano, controles de notificación y políticas de red/batería, **sin instalar Android Studio ni el SDK/Gradle completo en `air`** para preservar sus 69.5 GB libres y 8 GB de RAM.
* **Tareas:**
1. **En `air` (liviano):** Separar limpiamente el *core* portátil de Rust respecto a lo específico de escritorio, definir los *bindings* FFI/JNI (`uniffi` o `jni`), y estructurar el proyecto Android (Kotlin + Jetpack Compose + `Media3` / `MediaSession` para reproducción con pantalla bloqueada y controles del sistema).
2. Adaptar rutas de almacenamiento (caché LRU, base SQLite y descargas/pines offline para viajes) al almacenamiento de la app en Android, e integrar la autodetección del idioma del dispositivo (`Locale.getDefault()`) sincronizada con el *core* (`es`/`en` + selector manual).
3. Agregar controles de **ahorro de batería y datos**: *seedear* y descargar en lote solo por **Wi-Fi** o **cargando** (configurable), permitiendo reproducción/caché offline sin drenar batería ni datos móviles.
4. Soportar escaneo/pegado de **QR / Token** de la red de amigos en Android.
5. **Build automatizado del APK en CI:** Configurar un workflow de **GitHub Actions** (`.github/workflows/android.yml`) que instale Android SDK/NDK en la nube, compile el *core* Rust con `cargo-ndk` para `aarch64-linux-android`, ensamble el APK con Gradle y lo publique como *artifact* descargable (complementado con acceso móvil vía Web/PWA en la Fase 5).

* **Criterio de salida & Checkpoint:** El *core* y los bindings FFI compilan limpiamente en `air`, el workflow de GitHub Actions genera el APK instalable para Android con reproducción, pines offline, soporte multilingüe (`es`/`en`) y red de amigos; commit local y `push` a GitHub realizados. **Preguntar al usuario antes de iniciar la Fase 5.**

---

### Fase 5: Web / WebAssembly (WASM) + Control Remoto en Red Local

* **Objetivo:** Entregar una interfaz/cliente web apoyada en **WebAssembly (`wasm-pack` / `wasm-bindgen`)** y un *daemon/gateway* HTTP/WebSocket en el nodo Linux, respetando las limitaciones de seguridad/red de los navegadores modernos y habilitando el uso desde cualquier navegador o teléfono en la red local.
* **Tareas:**
1. Instalar de forma liviana el target `rustup target add wasm32-unknown-unknown` en `air`, y evaluar/documentar qué partes del *core* compilan directo a WASM vs. qué partes (como sockets UDP raw de BitTorrent/P2P) corren en el nodo/daemon Linux (`air` o `Kubuntu 26.04`).
2. Implementar el *daemon/gateway* HTTP/WebSocket + módulo WASM e interfaz web/PWA (reproducción en navegador, control remoto, navegación de catálogo/$HOME/playlists, visualización de estado/caché y pines), autodetectando el idioma del navegador (`navigator.language`) entre **Español** e **Inglés** con cambio manual en la Web UI.
3. Verificar *build* de WASM limpio y reproducibilidad local.

* **Criterio de salida & Checkpoint:** El módulo WASM, el *gateway* y la app web compilan y funcionan según el alcance acordado (incluyendo i18n `es`/`en` autodetectado); commit local y `push` a GitHub realizados. **Preguntar al usuario antes de iniciar la Fase 6.**

---

### Fase 6: Soporte para Windows y macOS

* **Objetivo:** Extender el soporte de escritorio a **Windows** y **macOS** aprovechando el *workspace* modular en Rust.
* **Tareas:**
1. Resolver particularidades de rutas de archivos (incluyendo auto-escaneo de `$HOME` / `%USERPROFILE%`), *backend* de audio, autodetección nativa del idioma del sistema operativo (`es`/`en`) en Windows y macOS, y *build* para `x86_64-pc-windows-msvc` (o `gnu`) y `aarch64-apple-darwin` / `x86_64-apple-darwin`.
2. Configurar/completar workflows automatizados en GitHub Actions (`.github/workflows/`) para verificar compilación y *tests* en Linux, Windows y macOS (sumando las validaciones de Android/WASM existentes).
3. Actualizar `README.md` con instrucciones de instalación y uso por plataforma.

* **Criterio de salida & Checkpoint:** Los *builds* y pruebas multiplataforma están configurados y pasan; commit local y `push` a GitHub realizados. **Preguntar al usuario si desea agregar mejoras o nuevas fases.**
