# rust-audio-p2p

Plataforma de audio distribuida en **Rust** que unifica **biblioteca local**, **streaming/descarga BitTorrent (`librqbit`)**, **caché LRU + pines offline para viajes**, y **red privada entre amigos sin servidor central (`iroh`)**.

Distributed audio platform in **Rust** unifying **local audio libraries**, **BitTorrent streaming/downloading (`librqbit`)**, **LRU cache + offline travel pins**, and **serverless friend-to-friend P2P networking (`iroh`)**.

---

## Instalación y Actualización Automática (`APT / .deb` y `.AppImage`)

### Opción 1: Paquete `.deb` (Ubuntu / Kubuntu) con actualización automática por `apt`
Descargá el último `.deb` desde [GitHub Releases](https://github.com/niunmango/rust-audio-p2p/releases/latest) (o desde [GitHub Pages](https://niunmango.github.io/rust-audio-p2p/)) e instalalo:
```bash
sudo apt install ./rust-audio-p2p_latest_amd64.deb
```
El propio paquete `.deb` configura `/etc/apt/sources.list.d/rust-audio-p2p.list` apuntando al repositorio APT en GitHub Pages. A partir de ese momento, cada vez que haya una nueva versión en `main`, se actualiza automáticamente ejecutando:
```bash
sudo apt update && sudo apt upgrade
```

### Opción 2: Portable `.AppImage` (cualquier distribución Linux `x86_64`)
```bash
chmod +x rust-audio-p2p-latest-x86_64.AppImage
./rust-audio-p2p-latest-x86_64.AppImage
```

---

## Arquitectura del Workspace / Workspace Architecture

- `crates/core` (`rust-audio-core`): Modelos de dominio, SQLite (`bundled`) + FTS5, i18n autodetectado (`es`/`en`), caché LRU, pines offline, cola y prefetch.
- `crates/source-local` (`rust-audio-source-local`): Auto-escaneo incremental de `$HOME` al iniciar, filtros de exclusión, lectura de tags/portadas (`lofty`), monitoreo con `notify` y lectura con *seek*.
- `crates/source-torrent` (`rust-audio-source-torrent`): Motor BitTorrent (`librqbit` con `rustls` en puertos `4240-4260`) y *trait* `TorrentSearchProvider` con búsqueda paralela Torznab/Jackett e Internet Archive.
- `crates/net-friends` (`rust-audio-net-friends`): Red privada P2P con `iroh` (Fase 3).
- `crates/player` (`rust-audio-player`): Reproductor (`rodio`/`symphonia`) con soporte PipeWire/PulseAudio/ALSA, *auto-next* y fallback headless `NullSink`.
- `bins/tui` (`rust-audio-tui`): Interfaz de terminal (`ratatui`) multilingüe (`es`/`en`) con mouse, *bracketed paste* y log de búsqueda en vivo.

---

## Compilación desde el Código / Build from Source

### Requisitos / Prerequisites (Ubuntu / Kubuntu)
```bash
sudo apt-get update && sudo apt-get install -y pkg-config libasound2-dev fakeroot
```

### Compilar y probar / Build & Test
```bash
cargo test --workspace
cargo build --release -p rust-audio-tui
./scripts/package-linux.sh
```

---

## Seguridad y privacidad / Security & privacy

- **Variables de entorno / Environment variables**:
  - `JACKETT_URL`: URL base de Jackett (por defecto `http://localhost:9117`). Como Torznab envía la `apikey` en la *query string*, usá `localhost` o **HTTPS** en despliegues remotos. / Base URL for Jackett (default `http://localhost:9117`). Since Torznab sends `apikey` in the query string, prefer `localhost` or **HTTPS** remotely.
  - `JACKETT_API_KEY`: clave API explícita de Jackett. / Explicit Jackett API key.
  - `JACKETT_CONFIG`: ruta explícita a `ServerConfig.json` (alternativa a `~/media-stack/...` o `~/.config/Jackett/ServerConfig.json`). / Explicit path to `ServerConfig.json`.
  - `INCLUDE_LAN_JACKETT=1`: solo para builds locales privados con `./scripts/package-linux.sh`; por defecto **desactivado** para no incrustar tu API key LAN en `.deb`/`.AppImage`. / Opt-in for private local builds only; disabled by default so your local Jackett API key is never embedded in distributed packages.
- **Exposición de IP en BitTorrent / BitTorrent IP exposure**: `librqbit` escucha en los puertos `4240–4260` con DHT habilitado y comparte piezas (*seeding*), por lo que tu IP pública es visible para los pares del enjambre. / `librqbit` listens on ports `4240–4260` with DHT enabled and seeds pieces, exposing your IP address to peers.
- **Permisos locales / Local permissions**: en sistemas Unix, `data_dir` se restringe a `0700` y `library.db` (donde puede guardarse `jackett_api_key`) a `0600`. / On Unix systems, `data_dir` is restricted to `0700` and `library.db` to `0600`.
- **Verificación de integridad / Integrity verification**: los artefactos `.deb` y `.AppImage` publican `SHA256SUMS.txt` (`sha256sum -c --ignore-missing SHA256SUMS.txt`). / `.deb` and `.AppImage` artifacts include `SHA256SUMS.txt` (`sha256sum -c --ignore-missing SHA256SUMS.txt`).
- **Firma GPG opcional del repositorio APT / Optional APT GPG signing**: configurando `APT_GPG_PRIVATE_KEY` (y opcionalmente `APT_GPG_PASSPHRASE`) en GitHub Secrets, CI firma `Release.gpg`/`InRelease` y el paquete `.deb` reemplaza automáticamente `trusted=yes` por `signed-by=/usr/share/keyrings/rust-audio-p2p.gpg`. / When `APT_GPG_PRIVATE_KEY` (and optionally `APT_GPG_PASSPHRASE`) is configured in GitHub Secrets, CI signs `Release.gpg`/`InRelease` and the `.deb` automatically switches from `trusted=yes` to `signed-by=/usr/share/keyrings/rust-audio-p2p.gpg`.

## Licencia / License
MIT — ver [LICENSE](LICENSE).
