use crate::models::Track;

#[derive(Debug, Default, Clone)]
pub struct PlaybackQueue {
    items: Vec<Track>,
    current_index: Option<usize>,
}

impl PlaybackQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_queue(&mut self, tracks: Vec<Track>, start_index: usize) {
        self.items = tracks;
        if self.items.is_empty() {
            self.current_index = None;
        } else {
            self.current_index = Some(start_index.min(self.items.len() - 1));
        }
    }

    pub fn enqueue(&mut self, track: Track) {
        self.items.push(track);
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
    }

    pub fn play_now(&mut self, track: Track) {
        if let Some(pos) = self.items.iter().position(|t| t.id == track.id && track.id != 0) {
            self.current_index = Some(pos);
        } else {
            self.items.push(track);
            self.current_index = Some(self.items.len() - 1);
        }
    }

    pub fn current(&self) -> Option<&Track> {
        self.current_index.and_then(|idx| self.items.get(idx))
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    pub fn next(&mut self) -> Option<&Track> {
        let idx = self.current_index?;
        if idx + 1 < self.items.len() {
            self.current_index = Some(idx + 1);
            self.current()
        } else {
            None
        }
    }

    pub fn prev(&mut self) -> Option<&Track> {
        let idx = self.current_index?;
        if idx > 0 {
            self.current_index = Some(idx - 1);
            self.current()
        } else {
            self.current()
        }
    }

    pub fn peek_next_for_prefetch(&self) -> Option<&Track> {
        let idx = self.current_index?;
        self.items.get(idx + 1)
    }

    pub fn items(&self) -> &[Track] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TrackSource;

    fn dummy_track(id: i64, title: &str) -> Track {
        Track {
            id,
            title: title.to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            year: Some(2024),
            track_number: Some(id as u32),
            duration_secs: Some(180),
            file_size_bytes: 1024,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: format!("/tmp/{id}.flac"),
            },
            pinned: false,
        }
    }

    #[test]
    fn test_queue_and_prefetch() {
        let mut q = PlaybackQueue::new();
        q.set_queue(vec![dummy_track(1, "One"), dummy_track(2, "Two"), dummy_track(3, "Three")], 0);
        assert_eq!(q.current().unwrap().title, "One");
        assert_eq!(q.peek_next_for_prefetch().unwrap().title, "Two");

        assert_eq!(q.next().unwrap().title, "Two");
        assert_eq!(q.peek_next_for_prefetch().unwrap().title, "Three");

        assert_eq!(q.next().unwrap().title, "Three");
        assert!(q.peek_next_for_prefetch().is_none());

        assert_eq!(q.prev().unwrap().title, "Two");
    }
}
