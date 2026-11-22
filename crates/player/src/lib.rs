use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use rodio::{Decoder, OutputStream, Sink, Source};
use rust_audio_core::{Track, TrackSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBackendKind {
    NativeAlsaPipeWire,
    HeadlessNullSink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone)]
pub struct PlayerStatus {
    pub state: PlaybackState,
    pub backend: AudioBackendKind,
    pub current_track: Option<Track>,
    pub prefetch_track: Option<Track>,
    pub prefetched_bytes: usize,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub last_error: Option<String>,
}

struct SharedPlayerState {
    state: PlaybackState,
    backend: AudioBackendKind,
    current_track: Option<Track>,
    prefetch_track: Option<Track>,
    prefetched_bytes: usize,
    base_pos_secs: f64,
    play_started_at: Option<Instant>,
    duration_secs: Option<f64>,
    volume: f32,
    last_error: Option<String>,
    track_finished: bool,
}

enum PlayerCmd {
    Play {
        track: Track,
        next_prefetch: Option<Track>,
    },
    TogglePause,
    Pause,
    Resume,
    Stop,
    SeekTo(f64),
    SeekRelative(f64),
    SetVolume(f32),
    SetPrefetch(Option<Track>),
}

pub struct PlayerEngine {
    cmd_tx: Sender<PlayerCmd>,
    shared: Arc<Mutex<SharedPlayerState>>,
}

impl PlayerEngine {
    pub fn new() -> Self {
        Self::with_headless_override(std::env::var("RUST_AUDIO_HEADLESS").as_deref() == Ok("1"))
    }

    pub fn new_headless() -> Self {
        Self::with_headless_override(true)
    }

    fn with_headless_override(force_headless: bool) -> Self {
        let shared = Arc::new(Mutex::new(SharedPlayerState {
            state: PlaybackState::Stopped,
            backend: if force_headless {
                AudioBackendKind::HeadlessNullSink
            } else {
                AudioBackendKind::NativeAlsaPipeWire
            },
            current_track: None,
            prefetch_track: None,
            prefetched_bytes: 0,
            base_pos_secs: 0.0,
            play_started_at: None,
            duration_secs: None,
            volume: 1.0,
            last_error: None,
            track_finished: false,
        }));

        let (cmd_tx, cmd_rx) = mpsc::channel::<PlayerCmd>();
        let worker_state = Arc::clone(&shared);

        std::thread::spawn(move || {
            let mut native_stream: Option<(OutputStream, Sink)> = if force_headless {
                None
            } else {
                match OutputStream::try_default() {
                    Ok((stream, handle)) => match Sink::try_new(&handle) {
                        Ok(sink) => Some((stream, sink)),
                        Err(_) => None,
                    },
                    Err(_) => None,
                }
            };

            if native_stream.is_none() {
                if let Ok(mut g) = worker_state.lock() {
                    g.backend = AudioBackendKind::HeadlessNullSink;
                }
            }

            // Track whether the active track is currently backed by the native sink vs headless clock.
            let mut using_native_for_track = false;

            loop {
                match cmd_rx.recv_timeout(Duration::from_millis(120)) {
                    Ok(cmd) => match cmd {
                        PlayerCmd::Play {
                            track,
                            next_prefetch,
                        } => {
                            if let Some((_, sink)) = native_stream.as_mut() {
                                sink.stop();
                            }
                            using_native_for_track = false;

                            let mut decoded_duration = track.duration_secs.map(|d| d as f64);
                            let mut err_msg = None;

                            if let Some(playable) = track.source.playable_path() {
                                if let Some((_, sink)) = native_stream.as_mut() {
                                    match File::open(playable)
                                        .map_err(anyhow::Error::from)
                                        .and_then(|f| Decoder::new(BufReader::new(f)).map_err(anyhow::Error::from))
                                    {
                                        Ok(source) => {
                                            if let Some(dur) = source.total_duration() {
                                                decoded_duration = Some(dur.as_secs_f64());
                                            }
                                            let vol = worker_state.lock().map(|g| g.volume).unwrap_or(1.0);
                                            sink.set_volume(vol);
                                            sink.append(source);
                                            sink.play();
                                            using_native_for_track = true;
                                        }
                                        Err(e) => {
                                            err_msg = Some(format!("Decoder fallback (NullSink): {e}"));
                                        }
                                    }
                                }
                            } else {
                                err_msg = Some("Torrent file not yet cached locally".to_string());
                            }

                            let prefetched_bytes = prefetch_next_bytes(next_prefetch.as_ref());

                            if let Ok(mut g) = worker_state.lock() {
                                g.current_track = Some(track);
                                g.prefetch_track = next_prefetch;
                                g.prefetched_bytes = prefetched_bytes;
                                g.duration_secs = decoded_duration;
                                g.base_pos_secs = 0.0;
                                g.play_started_at = Some(Instant::now());
                                g.state = PlaybackState::Playing;
                                g.last_error = err_msg;
                                g.track_finished = false;
                            }
                        }
                        PlayerCmd::TogglePause => {
                            let is_playing = worker_state
                                .lock()
                                .map(|g| g.state == PlaybackState::Playing)
                                .unwrap_or(false);
                            if is_playing {
                                pause_playback(&worker_state, native_stream.as_ref(), using_native_for_track);
                            } else {
                                resume_playback(&worker_state, native_stream.as_ref(), using_native_for_track);
                            }
                        }
                        PlayerCmd::Pause => {
                            pause_playback(&worker_state, native_stream.as_ref(), using_native_for_track);
                        }
                        PlayerCmd::Resume => {
                            resume_playback(&worker_state, native_stream.as_ref(), using_native_for_track);
                        }
                        PlayerCmd::Stop => {
                            if let Some((_, sink)) = native_stream.as_ref() {
                                sink.stop();
                            }
                            using_native_for_track = false;
                            if let Ok(mut g) = worker_state.lock() {
                                g.state = PlaybackState::Stopped;
                                g.base_pos_secs = 0.0;
                                g.play_started_at = None;
                                g.track_finished = false;
                            }
                        }
                        PlayerCmd::SeekTo(target_secs) => {
                            seek_absolute(
                                &worker_state,
                                native_stream.as_ref(),
                                using_native_for_track,
                                target_secs,
                            );
                        }
                        PlayerCmd::SeekRelative(delta_secs) => {
                            let cur = compute_current_pos(&worker_state, native_stream.as_ref(), using_native_for_track);
                            seek_absolute(
                                &worker_state,
                                native_stream.as_ref(),
                                using_native_for_track,
                                (cur + delta_secs).max(0.0),
                            );
                        }
                        PlayerCmd::SetVolume(vol) => {
                            let clamped = vol.clamp(0.0, 1.5);
                            if let Some((_, sink)) = native_stream.as_ref() {
                                sink.set_volume(clamped);
                            }
                            if let Ok(mut g) = worker_state.lock() {
                                g.volume = clamped;
                            }
                        }
                        PlayerCmd::SetPrefetch(next) => {
                            let bytes = prefetch_next_bytes(next.as_ref());
                            if let Ok(mut g) = worker_state.lock() {
                                g.prefetch_track = next;
                                g.prefetched_bytes = bytes;
                            }
                        }
                    },
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if using_native_for_track {
                            if let Some((_, sink)) = native_stream.as_ref() {
                                if let Ok(mut g) = worker_state.lock() {
                                    if g.state == PlaybackState::Playing {
                                        g.base_pos_secs = sink.get_pos().as_secs_f64();
                                        g.play_started_at = Some(Instant::now());
                                        if sink.empty() {
                                            g.state = PlaybackState::Stopped;
                                            g.play_started_at = None;
                                            g.track_finished = true;
                                        }
                                    }
                                }
                            }
                        } else if let Ok(mut g) = worker_state.lock() {
                            if g.state == PlaybackState::Playing {
                                if let (Some(started), Some(dur)) = (g.play_started_at, g.duration_secs) {
                                    let pos = g.base_pos_secs + started.elapsed().as_secs_f64();
                                    if pos >= dur && dur > 0.0 {
                                        g.base_pos_secs = dur;
                                        g.state = PlaybackState::Stopped;
                                        g.play_started_at = None;
                                        g.track_finished = true;
                                    }
                                }
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Self { cmd_tx, shared }
    }

    pub fn play_track(&self, track: Track, next_for_prefetch: Option<Track>) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::Play {
                track,
                next_prefetch: next_for_prefetch,
            })
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn toggle_pause(&self) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::TogglePause)
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn pause(&self) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::Pause)
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn resume(&self) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::Resume)
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn stop(&self) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::Stop)
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::SeekTo(seconds))
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn seek_relative(&self, delta_seconds: f64) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::SeekRelative(delta_seconds))
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn set_volume(&self, vol: f32) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::SetVolume(vol))
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn set_prefetch(&self, next: Option<Track>) -> Result<()> {
        self.cmd_tx
            .send(PlayerCmd::SetPrefetch(next))
            .map_err(|e| anyhow::anyhow!("player worker unavailable: {e}"))
    }

    pub fn take_track_finished(&self) -> bool {
        if let Ok(mut g) = self.shared.lock() {
            let finished = g.track_finished;
            g.track_finished = false;
            finished
        } else {
            false
        }
    }

    pub fn status(&self) -> PlayerStatus {
        let g = self.shared.lock().expect("player mutex poisoned");
        let position_secs = match g.state {
            PlaybackState::Playing => {
                let elapsed = g
                    .play_started_at
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let pos = g.base_pos_secs + elapsed;
                if let Some(dur) = g.duration_secs {
                    pos.min(dur)
                } else {
                    pos
                }
            }
            PlaybackState::Paused | PlaybackState::Stopped => g.base_pos_secs,
        };
        PlayerStatus {
            state: g.state,
            backend: g.backend,
            current_track: g.current_track.clone(),
            prefetch_track: g.prefetch_track.clone(),
            prefetched_bytes: g.prefetched_bytes,
            position_secs,
            duration_secs: g.duration_secs,
            volume: g.volume,
            last_error: g.last_error.clone(),
        }
    }
}

impl Default for PlayerEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn compute_current_pos(
    shared: &Arc<Mutex<SharedPlayerState>>,
    native: Option<&(OutputStream, Sink)>,
    using_native: bool,
) -> f64 {
    if using_native {
        if let Some((_, sink)) = native {
            return sink.get_pos().as_secs_f64();
        }
    }
    if let Ok(g) = shared.lock() {
        let elapsed = if g.state == PlaybackState::Playing {
            g.play_started_at
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0)
        } else {
            0.0
        };
        g.base_pos_secs + elapsed
    } else {
        0.0
    }
}

fn pause_playback(
    shared: &Arc<Mutex<SharedPlayerState>>,
    native: Option<&(OutputStream, Sink)>,
    using_native: bool,
) {
    if using_native {
        if let Some((_, sink)) = native {
            sink.pause();
        }
    }
    if let Ok(mut g) = shared.lock() {
        if g.state == PlaybackState::Playing {
            if let Some(started) = g.play_started_at.take() {
                g.base_pos_secs += started.elapsed().as_secs_f64();
            }
            g.state = PlaybackState::Paused;
        }
    }
}

fn resume_playback(
    shared: &Arc<Mutex<SharedPlayerState>>,
    native: Option<&(OutputStream, Sink)>,
    using_native: bool,
) {
    if using_native {
        if let Some((_, sink)) = native {
            sink.play();
        }
    }
    if let Ok(mut g) = shared.lock() {
        if g.state == PlaybackState::Paused {
            g.play_started_at = Some(Instant::now());
            g.state = PlaybackState::Playing;
        }
    }
}

fn seek_absolute(
    shared: &Arc<Mutex<SharedPlayerState>>,
    native: Option<&(OutputStream, Sink)>,
    using_native: bool,
    target_secs: f64,
) {
    let mut clamped = target_secs.max(0.0);
    if let Ok(g) = shared.lock() {
        if let Some(dur) = g.duration_secs {
            clamped = clamped.min(dur);
        }
    }
    let mut native_err = None;
    if using_native {
        if let Some((_, sink)) = native {
            if let Err(e) = sink.try_seek(Duration::from_secs_f64(clamped)) {
                native_err = Some(format!("seek warning: {e}"));
            }
        }
    }
    if let Ok(mut g) = shared.lock() {
        g.base_pos_secs = clamped;
        if g.state == PlaybackState::Playing {
            g.play_started_at = Some(Instant::now());
        }
        if native_err.is_some() {
            g.last_error = native_err;
        }
    }
}

fn prefetch_next_bytes(next: Option<&Track>) -> usize {
    let Some(track) = next else {
        return 0;
    };
    match &track.source {
        TrackSource::LocalFile { path } => read_head_buffer(Path::new(path), 256 * 1024),
        TrackSource::Torrent {
            cached_path: Some(path),
            ..
        } => read_head_buffer(Path::new(path), 256 * 1024),
        TrackSource::Torrent {
            cached_path: None, ..
        } => 0,
    }
}

fn read_head_buffer(path: &Path, max_bytes: usize) -> usize {
    let Ok(mut f) = File::open(path) else {
        return 0;
    };
    let mut buf = vec![0u8; max_bytes.min(64 * 1024)];
    let mut total = 0usize;
    while total < max_bytes {
        match f.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => total += n,
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_headless_player_play_seek_pause_and_prefetch() {
        let temp_next = std::env::temp_dir().join(format!("rust_audio_prefetch_{}.bin", std::process::id()));
        std::fs::write(&temp_next, vec![7u8; 4096]).unwrap();

        let t1 = Track {
            id: 1,
            title: "Track N".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            year: Some(2025),
            track_number: Some(1),
            duration_secs: Some(120),
            file_size_bytes: 1024,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: "/tmp/nonexistent_n.flac".to_string(),
            },
            pinned: false,
        };
        let t2 = Track {
            id: 2,
            title: "Track N+1".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            year: Some(2025),
            track_number: Some(2),
            duration_secs: Some(150),
            file_size_bytes: 4096,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: temp_next.to_string_lossy().to_string(),
            },
            pinned: false,
        };

        let player = PlayerEngine::new_headless();
        player.play_track(t1, Some(t2)).unwrap();
        std::thread::sleep(Duration::from_millis(80));

        let st = player.status();
        assert_eq!(st.backend, AudioBackendKind::HeadlessNullSink);
        assert_eq!(st.state, PlaybackState::Playing);
        assert_eq!(st.prefetch_track.as_ref().unwrap().title, "Track N+1");
        assert_eq!(st.prefetched_bytes, 4096);

        player.seek_to(45.0).unwrap();
        player.set_volume(0.75).unwrap();
        player.pause().unwrap();
        std::thread::sleep(Duration::from_millis(60));

        let st2 = player.status();
        assert_eq!(st2.state, PlaybackState::Paused);
        assert!((st2.position_secs - 45.0).abs() < 1.0);
        assert!((st2.volume - 0.75).abs() < 0.01);

        let _ = std::fs::remove_file(&temp_next);
    }

    #[test]
    fn test_headless_player_auto_next_signal_on_finish() {
        let t_short = Track {
            id: 99,
            title: "Short".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            year: Some(2025),
            track_number: Some(1),
            duration_secs: Some(1),
            file_size_bytes: 128,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: "/tmp/nonexistent_short.flac".to_string(),
            },
            pinned: false,
        };
        let player = PlayerEngine::new_headless();
        player.play_track(t_short, None).unwrap();
        player.seek_to(0.95).unwrap();
        std::thread::sleep(Duration::from_millis(260));
        assert!(player.take_track_finished(), "track_finished flag should trigger when playback reaches duration");
        assert!(!player.take_track_finished(), "take_track_finished should consume the flag");
    }
}
