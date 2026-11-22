# rust-audio-p2p

Plataforma de audio distribuida en **Rust** que unifica **biblioteca local**, **streaming/descarga BitTorrent (`librqbit`)**, **caché LRU + pines offline para viajes**, y **red privada entre amigos sin servidor central (`iroh`)**.

Distributed audio platform in **Rust** unifying **local audio libraries**, **BitTorrent streaming/downloading (`librqbit`)**, **LRU cache + offline travel pins**, and **serverless friend-to-friend P2P networking (`iroh`)**.

---

## Arquitectura del Workspace / Workspace Architecture

- `crates/core` (`rust-audio-core`): Modelos de dominio, SQLite (`bundled`) + FTS5, i18n autodetectado (`es`/`en`), caché LRU, pines offline, cola y prefetch.
- `crates/source-local` (`rust-audio-source-local`): Auto-escaneo asíncrono de `$HOME` al iniciar, filtros de exclusión, lectura de tags/portadas (`lofty`), monitoreo incremental (`notify`) y lectura con *seek*.
- `crates/source-torrent` (`rust-audio-source-torrent`): Motor BitTorrent (`librqbit` con `rustls` en puertos `4240-4260`) y *trait* `TorrentSearchProvider` con adaptador Torznab/Jackett.
- `crates/net-friends` (`rust-audio-net-friends`): Red privada P2P con `iroh` (Fase 3).
- `crates/player` (`rust-audio-player`): Reproductor (`rodio`/`symphonia`) con soporte PipeWire/PulseAudio/ALSA y fallback headless `NullSink`.
- `bins/tui` (`rust-audio-tui`): Interfaz de terminal (`ratatui`) multilingüe (`es`/`en`).

---

## Compilación y Empaquetado en Linux / Building & Packaging on Linux

### Requisitos / Prerequisites (Ubuntu / Kubuntu)
```bash
sudo apt-get update && sudo apt-get install -y pkg-config libasound2-dev
```

### Compilar y probar / Build & Test
```bash
cargo check --workspace
cargo test --workspace
cargo run -p rust-audio-tui
```

### Generar `.deb` y `.AppImage` / Build `.deb` and `.AppImage`
```bash
./scripts/package-linux.sh
```
Genera en `dist/`:
- `rust-audio-p2p_0.1.0_amd64.deb` (`sudo apt install ./dist/rust-audio-p2p_0.1.0_amd64.deb`)
- `rust-audio-p2p-0.1.0-x86_64.AppImage` (`chmod +x dist/*.AppImage && ./dist/rust-audio-p2p-0.1.0-x86_64.AppImage`)

---

## Seguridad y privacidad / Security & privacy

- **Variables de entorno / Environment variables**:
  - `JACKETT_URL`: URL base de Jackett (por defecto `http://localhost:9117`). Como Torznab envía la `apikey` en la *query string*, se recomienda usar `localhost` o **HTTPS** en despliegues remotos.
  - `JACKETT_API_KEY`: clave API de Jackett (alternativa explícita a la autodetección de configuración).
  - `JACKETT_CONFIG`: ruta explícita a `ServerConfig.json` si no se encuentra en `$XDG_CONFIG_HOME/Jackett/ServerConfig.json` o `~/.config/Jackett/ServerConfig.json`.
  - `RUST_AUDIO_SELFTEST_MUSIC_DIR`: directorio opcional de música para `--self-test` cuando no se pasa `--scan-dir`.
- **Exposición de IP en BitTorrent / BitTorrent IP exposure**: el motor `librqbit` escucha en los puertos `4240–4260` con DHT habilitado y comparte piezas con pares (*seeding*), por lo que tu dirección IP pública será visible para los pares del enjambre. / `librqbit` listens on ports `4240–4260` with DHT enabled and seeds pieces to peers, so your IP address is visible to the swarm.
- **Permisos locales / Local permissions**: el directorio de datos (`library.db`) se crea con permisos `0700` en sistemas Unix. / The data directory (`library.db`) is created with `0700` permissions on Unix systems.

## Licencia / License
MIT — ver [LICENSE](LICENSE).
