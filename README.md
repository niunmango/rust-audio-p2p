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

## Licencia / License
MIT — ver [LICENSE](LICENSE).
