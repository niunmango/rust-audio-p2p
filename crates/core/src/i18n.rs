use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Es,
    En,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Language::Es => "es",
            Language::En => "en",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Language::Es => "Español",
            Language::En => "English",
        }
    }

    pub fn from_locale_tag(tag: &str) -> Option<Self> {
        let norm = tag.trim().to_ascii_lowercase();
        if norm.starts_with("es") {
            Some(Language::Es)
        } else if norm.starts_with("en") {
            Some(Language::En)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePreference {
    Auto,
    Es,
    En,
}

impl LanguagePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            LanguagePreference::Auto => "auto",
            LanguagePreference::Es => "es",
            LanguagePreference::En => "en",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "es" | "spanish" | "español" => LanguagePreference::Es,
            "en" | "english" | "inglés" => LanguagePreference::En,
            _ => LanguagePreference::Auto,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            LanguagePreference::Auto => LanguagePreference::Es,
            LanguagePreference::Es => LanguagePreference::En,
            LanguagePreference::En => LanguagePreference::Auto,
        }
    }

    pub fn resolve(self) -> Language {
        match self {
            LanguagePreference::Auto => detect_system_language(),
            LanguagePreference::Es => Language::Es,
            LanguagePreference::En => Language::En,
        }
    }
}

pub fn detect_system_language() -> Language {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            if let Some(lang) = Language::from_locale_tag(&val) {
                return lang;
            }
        }
    }
    if let Some(locale) = sys_locale::get_locale() {
        if let Some(lang) = Language::from_locale_tag(&locale) {
            return lang;
        }
    }
    Language::Es
}

#[derive(Debug, Clone)]
pub struct I18n {
    pub pref: LanguagePreference,
    pub lang: Language,
}

impl I18n {
    pub fn new(pref: LanguagePreference) -> Self {
        Self {
            pref,
            lang: pref.resolve(),
        }
    }

    pub fn set_preference(&mut self, pref: LanguagePreference) {
        self.pref = pref;
        self.lang = pref.resolve();
    }

    pub fn cycle_language(&mut self) {
        let next = self.pref.cycle();
        self.set_preference(next);
    }

    fn t(&self, es: &'static str, en: &'static str) -> &'static str {
        match self.lang {
            Language::Es => es,
            Language::En => en,
        }
    }

    pub fn app_title(&self) -> &'static str {
        "rust-audio-p2p"
    }

    pub fn tab_library(&self) -> &'static str {
        self.t("1: Biblioteca", "1: Library")
    }

    pub fn tab_search(&self) -> &'static str {
        self.t("2: Buscar (FTS5)", "2: Search (FTS5)")
    }

    pub fn tab_playlists(&self) -> &'static str {
        self.t("3: Playlists & Viaje", "3: Playlists & Travel")
    }

    pub fn tab_torrents(&self) -> &'static str {
        self.t("4: Torrents / Jackett", "4: Torrents / Jackett")
    }

    pub fn tab_cache_settings(&self) -> &'static str {
        self.t("5: Caché LRU & Ajustes", "5: LRU Cache & Settings")
    }

    pub fn tab_friends(&self) -> &'static str {
        self.t("6: Red P2P / Nodos", "6: P2P Net / Nodes")
    }

    pub fn col_title(&self) -> &'static str {
        self.t("Título", "Title")
    }

    pub fn col_artist(&self) -> &'static str {
        self.t("Artista", "Artist")
    }

    pub fn col_album(&self) -> &'static str {
        self.t("Álbum", "Album")
    }

    pub fn col_format(&self) -> &'static str {
        self.t("Fmt", "Fmt")
    }

    pub fn col_pin(&self) -> &'static str {
        self.t("Pin", "Pin")
    }

    pub fn scan_idle(&self) -> &'static str {
        self.t("Escaneo: inactivo", "Scan: idle")
    }

    pub fn scan_running(&self, dirs: usize, found: usize) -> String {
        match self.lang {
            Language::Es => format!("⟳ Escaneando $HOME ({dirs} dirs, {found} pistas)..."),
            Language::En => format!("⟳ Scanning $HOME ({dirs} dirs, {found} tracks)..."),
        }
    }

    pub fn scan_banner_running(&self, dirs: usize, found: usize, indexed: usize) -> String {
        match self.lang {
            Language::Es => format!(
                "⟳ Escaneo inicial en progreso en segundo plano: {dirs} carpetas revisadas | {found} archivos de audio | {indexed} indexados..."
            ),
            Language::En => format!(
                "⟳ Initial scan in progress in background: {dirs} folders checked | {found} audio files | {indexed} indexed..."
            ),
        }
    }

    pub fn loading_title(&self) -> &'static str {
        self.t(
            "Iniciando rust-audio-p2p...",
            "Starting rust-audio-p2p...",
        )
    }

    pub fn loading_subtitle(&self) -> &'static str {
        self.t(
            "Cargando biblioteca SQLite y preparando escaneo incremental de $HOME en segundo plano...",
            "Loading SQLite library and starting background incremental $HOME scan...",
        )
    }

    pub fn auto_next_label(&self, enabled: bool) -> &'static str {
        match (self.lang, enabled) {
            (Language::Es, true) => "Auto-Sig: ON [A]",
            (Language::Es, false) => "Auto-Sig: OFF [A]",
            (Language::En, true) => "Auto-Next: ON [A]",
            (Language::En, false) => "Auto-Next: OFF [A]",
        }
    }

    pub fn scan_completed(&self, indexed: usize) -> String {
        match self.lang {
            Language::Es => format!("Biblioteca lista: {indexed} pistas"),
            Language::En => format!("Library ready: {indexed} tracks"),
        }
    }

    pub fn status_playing(&self) -> &'static str {
        self.t("▶ REPRODUCIENDO", "▶ PLAYING")
    }

    pub fn status_paused(&self) -> &'static str {
        self.t("⏸ PAUSA", "⏸ PAUSED")
    }

    pub fn status_stopped(&self) -> &'static str {
        self.t("■ DETENIDO", "■ STOPPED")
    }

    pub fn audio_backend_native(&self) -> &'static str {
        self.t("Audio: Nativo (ALSA/PipeWire)", "Audio: Native (ALSA/PipeWire)")
    }

    pub fn audio_backend_headless(&self) -> &'static str {
        self.t("Audio: NullSink (Headless)", "Audio: NullSink (Headless)")
    }

    pub fn no_track_playing(&self) -> &'static str {
        self.t("Sin pista seleccionada", "No track selected")
    }

    pub fn prefetch_label(&self, title: Option<&str>) -> String {
        match (self.lang, title) {
            (Language::Es, Some(t)) => format!("Prefetch siguiente: {t}"),
            (Language::En, Some(t)) => format!("Prefetch next: {t}"),
            (Language::Es, None) => "Prefetch siguiente: ninguno".to_string(),
            (Language::En, None) => "Prefetch next: none".to_string(),
        }
    }

    pub fn search_prompt(&self) -> &'static str {
        self.t("Buscar (FTS5): ", "Search (FTS5): ")
    }

    pub fn search_hint(&self) -> &'static str {
        self.t(
            "Escribí directo aquí | ↑/↓: Mover | Enter: Reproducir | Esc: Salir de escritura (para usar a/p/1-5)",
            "Type directly here | ↑/↓: Move | Enter: Play | Esc: Exit typing (to use a/p/1-5)",
        )
    }

    pub fn playlists_header(&self) -> &'static str {
        self.t("Playlists", "Playlists")
    }

    pub fn playlists_guide(&self) -> &'static str {
        self.t(
            "Guía: [n] o clic en caja = Crear playlist | En Biblioteca(1)/Buscar(2) apretá [a] sobre un tema para agregarlo | [Tab] Cambiar panel | [Enter] Reproducir | [d] Quitar tema | [p] Pin Viaje",
            "Guide: [n] or click box = New playlist | In Library(1)/Search(2) press [a] on a track to add it | [Tab] Switch panel | [Enter] Play | [d] Remove track | [p] Travel Pin",
        )
    }

    pub fn playlist_tracks_header(&self, name: &str) -> String {
        match self.lang {
            Language::Es => format!("Pistas en '{name}'"),
            Language::En => format!("Tracks in '{name}'"),
        }
    }

    pub fn playlist_empty_hint(&self) -> &'static str {
        self.t(
            "Esta playlist está vacía. Andá a '1: Biblioteca' o '2: Buscar', seleccioná un tema y apretá [a] para agregarlo acá.",
            "This playlist is empty. Go to '1: Library' or '2: Search', select a track and press [a] to add it here.",
        )
    }

    pub fn playlist_picker_title(&self) -> &'static str {
        self.t("Agregar pista a Playlist (↑/↓/Clic + Enter | Esc: Cancelar)", "Add track to Playlist (↑/↓/Click + Enter | Esc: Cancel)")
    }

    pub fn playlist_picker_new_option(&self) -> &'static str {
        self.t("[+ Crear nueva Playlist y agregar...]", "[+ Create new Playlist and add...]")
    }

    pub fn removed_from_playlist(&self, title: &str, playlist: &str) -> String {
        match self.lang {
            Language::Es => format!("Quitada '{title}' de '{playlist}'"),
            Language::En => format!("Removed '{title}' from '{playlist}'"),
        }
    }

    pub fn pinned_offline_header(&self) -> &'static str {
        self.t("Pines Offline (Modo Viaje)", "Offline Pins (Travel Mode)")
    }

    pub fn torrents_search_prompt(&self) -> &'static str {
        self.t("Buscar Torrent (Jackett/Torznab): ", "Torrent Search (Jackett/Torznab): ")
    }

    pub fn torrents_magnet_prompt(&self) -> &'static str {
        self.t("Magnet / .torrent URI: ", "Magnet / .torrent URI: ")
    }

    pub fn torrents_searching_progress(&self, query: &str) -> String {
        match self.lang {
            Language::Es => format!("⟳ Buscando '{query}' en Jackett / proveedores..."),
            Language::En => format!("⟳ Searching '{query}' on Jackett / providers..."),
        }
    }

    pub fn torrents_active_header(&self) -> &'static str {
        self.t("Descargas y Seeding (librqbit, puertos 4240-4260)", "Downloads & Seeding (librqbit, ports 4240-4260)")
    }

    pub fn torrents_log_header(&self) -> &'static str {
        self.t("Log de Búsqueda en Vivo", "Live Search Log")
    }

    pub fn torrents_jackett_prompt(&self) -> &'static str {
        self.t("Jackett URL (Enter guardar): ", "Jackett URL (Enter save): ")
    }

    pub fn cache_header(&self) -> &'static str {
        self.t("Estado de Caché LRU y Preferencias", "LRU Cache Status & Preferences")
    }

    pub fn friends_local_header(&self) -> &'static str {
        self.t("Este Nodo P2P (Compartir Audio en Red / LAN)", "This P2P Node (Share Audio over Network / LAN)")
    }

    pub fn friends_connect_prompt(&self) -> &'static str {
        self.t(
            "[c] Conectar Nodo (Token rap2p:// o IP:Puerto): ",
            "[c] Connect Node (rap2p:// Token or IP:Port): ",
        )
    }

    pub fn friends_lan_share_label(&self, enabled: bool) -> &'static str {
        match (self.lang, enabled) {
            (Language::Es, true) => "Compartir en LAN [s]: ON",
            (Language::Es, false) => "Compartir en LAN [s]: OFF (Solo con Token)",
            (Language::En, true) => "LAN Share [s]: ON",
            (Language::En, false) => "LAN Share [s]: OFF (Token Only)",
        }
    }

    pub fn friends_actions_hint(&self) -> &'static str {
        self.t(
            "[c] Conectar | [d] Descubrir LAN | [s] LAN On/Off | [v] Ver QR | [Enter/DblClic] Reproducir/Cachear | [p] Pin | [a] +Playlist",
            "[c] Connect | [d] Discover LAN | [s] LAN On/Off | [v] Show QR | [Enter/DblClick] Play/Cache | [p] Pin | [a] +Playlist",
        )
    }

    pub fn friends_peers_header(&self) -> &'static str {
        self.t("Nodos Emparejados", "Paired Nodes")
    }

    pub fn friends_tracks_header(&self) -> &'static str {
        self.t("Catálogo Remoto P2P", "Remote P2P Catalog")
    }

    pub fn friends_log_header(&self) -> &'static str {
        self.t("Actividad P2P en Vivo", "Live P2P Activity")
    }

    pub fn friends_qr_title(&self) -> &'static str {
        self.t("Token / QR de Emparejamiento P2P (Esc: Cerrar)", "P2P Pairing Token / QR (Esc: Close)")
    }

    pub fn friends_scan_btn(&self) -> &'static str {
        self.t("[d] Buscar LAN", "[d] Scan LAN")
    }

    pub fn friends_no_peers_hint(&self) -> &'static str {
        self.t(
            "  (Sin nodos — usá [d] LAN o pegá token rap2p:// / IP)",
            "  (No peers — press [d] LAN or paste rap2p:// token / IP)",
        )
    }

    pub fn friends_catalog_suffix(&self) -> &'static str {
        self.t("☁=Stream/Cache | ✓=Cacheado", "☁=Stream/Cache | ✓=Cached")
    }

    pub fn language_label(&self) -> String {
        let mode = match self.pref {
            LanguagePreference::Auto => self.t("Auto", "Auto"),
            LanguagePreference::Es => "Español",
            LanguagePreference::En => "English",
        };
        match self.lang {
            Language::Es => format!("Idioma: {mode} (activo: {}) [tecla 'l' para cambiar]", self.lang.display_name()),
            Language::En => format!("Language: {mode} (active: {}) [press 'l' to switch]", self.lang.display_name()),
        }
    }

    pub fn help_footer(&self) -> &'static str {
        self.t(
            "1-6/Clic: Pestañas | Enter/DblClic: Reproducir | Space: Pausa | ←/→: Seek ±10s | +/-: Vol | n/b: Sig/Ant | A: Auto-Sig | a: +Playlist | p: Pin | r: Re-escanear | /: Buscar | l: Idioma | q: Salir",
            "1-6/Click: Tabs | Enter/DblClick: Play | Space: Pause | ←/→: Seek ±10s | +/-: Vol | n/b: Next/Prev | A: Auto-Next | a: +Playlist | p: Pin | r: Rescan | /: Search | l: Lang | q: Quit",
        )
    }

    pub fn playlist_created(&self, name: &str) -> String {
        match self.lang {
            Language::Es => format!("Playlist creada: {name}"),
            Language::En => format!("Playlist created: {name}"),
        }
    }

    pub fn added_to_playlist(&self, title: &str, playlist: &str) -> String {
        match self.lang {
            Language::Es => format!("Agregada '{title}' a '{playlist}'"),
            Language::En => format!("Added '{title}' to '{playlist}'"),
        }
    }

    pub fn pin_toggled(&self, title: &str, pinned: bool) -> String {
        match (self.lang, pinned) {
            (Language::Es, true) => format!("Pineada para viaje: {title}"),
            (Language::Es, false) => format!("Pin quitado: {title}"),
            (Language::En, true) => format!("Pinned for travel: {title}"),
            (Language::En, false) => format!("Unpinned: {title}"),
        }
    }

    pub fn cache_purged(&self, count: usize, freed_mb: f64) -> String {
        match self.lang {
            Language::Es => format!("Caché purgada: {count} entradas ({freed_mb:.2} MB liberados)"),
            Language::En => format!("Cache purged: {count} entries ({freed_mb:.2} MB freed)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locale_tag_parsing() {
        assert_eq!(Language::from_locale_tag("es_AR.UTF-8"), Some(Language::Es));
        assert_eq!(Language::from_locale_tag("es-ES"), Some(Language::Es));
        assert_eq!(Language::from_locale_tag("en_US.UTF-8"), Some(Language::En));
        assert_eq!(Language::from_locale_tag("en-GB"), Some(Language::En));
        assert_eq!(Language::from_locale_tag("fr_FR"), None);
    }

    #[test]
    fn test_i18n_switching() {
        let mut i18n = I18n::new(LanguagePreference::Es);
        assert_eq!(i18n.lang, Language::Es);
        assert_eq!(i18n.tab_library(), "1: Biblioteca");

        i18n.set_preference(LanguagePreference::En);
        assert_eq!(i18n.lang, Language::En);
        assert_eq!(i18n.tab_library(), "1: Library");

        i18n.cycle_language();
        assert_eq!(i18n.pref, LanguagePreference::Auto);
    }
}
