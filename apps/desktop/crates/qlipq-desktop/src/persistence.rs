//! Config, edits, and cache snapshots are written by one worker in submission order.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use qlipq_core::{config::AppConfig, config_json};

use crate::host::{self, EditStore, MediaCache};

enum Snapshot {
    Config(Box<AppConfig>),
    Edits(EditStore),
    MediaCache(MediaCache),
}

impl Snapshot {
    fn file(&self) -> &'static str {
        match self {
            Self::Config(_) => "config.json",
            Self::Edits(_) => "edits.json",
            Self::MediaCache(_) => "media-cache.json",
        }
    }

    fn write(self, dir: &Path) -> io::Result<()> {
        let name = self.file();
        let contents = match self {
            Self::Config(config) => config_json::serialize(&config),
            Self::Edits(edits) => serde_json::to_string(&edits).map_err(io::Error::other)?,
            Self::MediaCache(cache) => {
                host::serialize_media_cache(&cache).map_err(io::Error::other)?
            }
        };
        write_atomic(dir, name, &contents)
    }
}

#[derive(Debug)]
pub struct SaveResult {
    pub id: Option<u64>,
    pub file: &'static str,
    pub result: Result<(), String>,
}

pub struct Persistence {
    sender: Option<Sender<(u64, Snapshot)>>,
    next_receipt: AtomicU64,
    results: Receiver<SaveResult>,
    worker: Option<JoinHandle<()>>,
    stopped_reported: bool,
}

impl Persistence {
    pub fn new() -> Self {
        Self::at_path(host::data_dir())
    }

    pub(crate) fn at_path(dir: PathBuf) -> Self {
        Self::with_writer(move |snapshot| snapshot.write(&dir))
    }

    fn with_writer(mut write: impl FnMut(Snapshot) -> io::Result<()> + Send + 'static) -> Self {
        let (sender, requests) = mpsc::channel::<(u64, Snapshot)>();
        let (completion, results) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            for (id, snapshot) in requests {
                let file = snapshot.file();
                let result =
                    write(snapshot).map_err(|error| format!("Could not save {file}: {error}"));
                let _ = completion.send(SaveResult {
                    id: Some(id),
                    file,
                    result,
                });
            }
        });
        Self {
            sender: Some(sender),
            next_receipt: AtomicU64::new(1),
            results,
            worker: Some(worker),
            stopped_reported: false,
        }
    }

    pub fn save_config(&self, config: AppConfig) -> Result<u64, String> {
        self.submit(Snapshot::Config(Box::new(config)))
    }

    pub fn save_edits(&self, edits: EditStore) -> Result<u64, String> {
        self.submit(Snapshot::Edits(edits))
    }

    pub fn save_media_cache(&self, cache: MediaCache) -> Result<u64, String> {
        self.submit(Snapshot::MediaCache(cache))
    }

    fn submit(&self, snapshot: Snapshot) -> Result<u64, String> {
        let file = snapshot.file();
        let id = self.next_receipt.fetch_add(1, Ordering::Relaxed);
        self.sender
            .as_ref()
            .ok_or_else(|| format!("Could not save {file}: persistence worker stopped"))?
            .send((id, snapshot))
            .map_err(|_| format!("Could not save {file}: persistence worker stopped"))?;
        Ok(id)
    }

    pub fn take_results(&mut self) -> Vec<SaveResult> {
        let mut results = Vec::new();
        loop {
            match self.results.try_recv() {
                Ok(result) => results.push(result),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.stopped_reported {
                        results.push(SaveResult {
                            id: None,
                            file: "settings",
                            result: Err(
                                "Persistence worker stopped; changes may not be saved".into()
                            ),
                        });
                        self.stopped_reported = true;
                    }
                    break;
                }
            }
        }
        results
    }
}

impl Drop for Persistence {
    fn drop(&mut self) {
        // App shutdown closes the queue, so every submitted snapshot finishes before exit.
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("Persistence worker stopped before shutdown completed");
            }
        }
        for completion in self.results.try_iter() {
            if let Err(error) = completion.result {
                eprintln!("{error}");
            }
        }
    }
}

fn write_atomic(dir: &Path, name: &str, contents: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!("{name}.{}.{seq}.tmp", std::process::id()));
    let mut file = std::fs::File::create_new(&temp)?;
    let staged = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.sync_all());
    drop(file);
    staged
        .and_then(|_| std::fs::rename(&temp, dir.join(name)))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "qlipq-persistence-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQ.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn queued_snapshots_cannot_restore_a_removed_edit() {
        let dir = TestDir::new();
        let path = dir.0.clone();
        let (started, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let mut first = true;
        let persistence = Persistence::with_writer(move |snapshot| {
            if first {
                first = false;
                started.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            snapshot.write(&path)
        });
        let mut old = EditStore::new();
        old.insert("clip.mp4".into(), host::StoredEdit::default());
        persistence.save_edits(old).unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        persistence.save_edits(EditStore::new()).unwrap();
        release.send(()).unwrap();
        drop(persistence);
        let saved = std::fs::read_to_string(dir.0.join("edits.json")).unwrap();
        assert!(serde_json::from_str::<EditStore>(&saved)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn all_store_types_share_one_ordered_queue_and_drain_at_shutdown() {
        let dir = TestDir::new();
        let path = dir.0.clone();
        let (written, files) = mpsc::channel();
        let persistence = Persistence::with_writer(move |snapshot| {
            written.send(snapshot.file()).unwrap();
            snapshot.write(&path)
        });
        persistence.save_config(AppConfig::default()).unwrap();
        persistence.save_edits(EditStore::new()).unwrap();
        persistence.save_media_cache(MediaCache::new()).unwrap();
        let config = AppConfig {
            output_folder: "latest".into(),
            ..AppConfig::default()
        };
        persistence.save_config(config.clone()).unwrap();
        drop(persistence);
        assert_eq!(
            files.into_iter().collect::<Vec<_>>(),
            [
                "config.json",
                "edits.json",
                "media-cache.json",
                "config.json"
            ],
        );
        let saved = std::fs::read_to_string(dir.0.join("config.json")).unwrap();
        assert_eq!(config_json::parse(&saved), config);
        let cache = std::fs::read_to_string(dir.0.join("media-cache.json")).unwrap();
        assert_eq!(
            cache,
            host::serialize_media_cache(&MediaCache::new()).unwrap()
        );
    }

    #[test]
    fn io_failures_are_reported_and_do_not_stop_following_saves() {
        let dir = TestDir::new();
        std::fs::create_dir(dir.0.join("config.json")).unwrap();
        let persistence = Persistence::at_path(dir.0.clone());
        let failed_id = persistence.save_config(AppConfig::default()).unwrap();
        let saved_id = persistence.save_edits(EditStore::new()).unwrap();
        let failure = persistence
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(failure.file, "config.json");
        assert_eq!(failure.id, Some(failed_id));
        assert!(failure
            .result
            .unwrap_err()
            .contains("Could not save config.json"));
        let success = persistence
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(success.file, "edits.json");
        assert_eq!(success.id, Some(saved_id));
        assert!(success.result.is_ok());
        drop(persistence);
        assert!(dir.0.join("config.json").is_dir());
        assert!(dir.0.join("edits.json").is_file());
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
    }

    #[test]
    fn an_unwritable_data_directory_reports_an_error() {
        let dir = TestDir::new();
        let path = dir.0.join("not-a-directory");
        std::fs::write(&path, "occupied").unwrap();
        let persistence = Persistence::at_path(path);
        persistence.save_config(AppConfig::default()).unwrap();
        let completion = persistence
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(completion.result.is_err());
    }

    #[test]
    fn a_stopped_worker_reports_disconnection_once_and_rejects_new_saves() {
        let mut persistence = Persistence::with_writer(|_| panic!("test worker failure"));
        persistence.save_edits(EditStore::new()).unwrap();
        assert!(persistence.worker.take().unwrap().join().is_err());
        let results = persistence.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, None);
        assert!(results[0].result.is_err());
        assert!(persistence.take_results().is_empty());
        assert!(persistence.save_edits(EditStore::new()).is_err());
    }

    #[test]
    fn an_earlier_config_completion_does_not_match_a_pending_save_receipt() {
        let (entered, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let mut saves = 0;
        let mut persistence = Persistence::with_writer(move |_| {
            saves += 1;
            if saves == 2 {
                entered.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            Ok(())
        });
        let first = persistence.save_config(AppConfig::default()).unwrap();
        let requested = persistence.save_config(AppConfig::default()).unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let earlier = persistence.take_results();
        assert_eq!(earlier.len(), 1);
        assert_eq!(earlier[0].id, Some(first));
        assert_ne!(earlier[0].id, Some(requested));
        release.send(()).unwrap();
        let completed = persistence
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(completed.id, Some(requested));
        assert!(completed.result.is_ok());
    }
}
