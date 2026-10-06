use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Discovery {
    generation: u64,
    active: Option<(u64, Vec<String>)>,
    pending: Vec<String>,
    requested_at: Option<Instant>,
    next_revision: u64,
    files: HashMap<String, FileState>,
}

struct FileState {
    stamp: (i64, i64),
    revision: u64,
    probing: bool,
}

impl Discovery {
    pub fn request(&mut self, roots: Vec<String>) {
        if let Some((generation, active_roots)) = &self.active {
            if *generation == self.generation {
                self.pending.extend(active_roots.iter().cloned());
            }
        }
        self.generation += 1;
        self.pending.extend(roots);
        self.pending.sort();
        self.pending.dedup();
        self.requested_at = Some(Instant::now());
    }

    pub fn reset(&mut self, roots: Vec<String>) {
        self.generation += 1;
        self.pending = roots;
        self.requested_at = Some(Instant::now());
    }

    pub fn begin(&mut self, immediate: bool) -> Option<(u64, Vec<String>)> {
        if self.active.is_some() || self.pending.is_empty() {
            return None;
        }
        if !immediate && self.requested_at.is_some_and(|at| at.elapsed() < DEBOUNCE) {
            return None;
        }
        let roots = std::mem::take(&mut self.pending);
        self.active = Some((self.generation, roots.clone()));
        Some((self.generation, roots))
    }

    pub fn finish(&mut self, generation: u64) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|(id, _)| *id == generation)
        {
            self.active = None;
        }
        self.is_current(generation)
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    pub fn probe(&mut self, path: &str, size: i64, modified_ms: i64, cached: bool) -> Option<u64> {
        let stamp = (size, modified_ms);
        let changed = self.files.get(path).is_none_or(|file| file.stamp != stamp);
        if changed {
            self.next_revision += 1;
            self.files.insert(
                path.into(),
                FileState {
                    stamp,
                    revision: self.next_revision,
                    probing: false,
                },
            );
        }
        let file = self.files.get_mut(path)?;
        if cached || file.probing {
            return None;
        }
        file.probing = true;
        Some(file.revision)
    }

    pub fn finish_probe(&mut self, path: &str, revision: u64) -> bool {
        let Some(file) = self.files.get_mut(path) else {
            return false;
        };
        if file.revision != revision || !file.probing {
            return false;
        }
        file.probing = false;
        true
    }

    pub fn remove(&mut self, path: &str) {
        self.files.remove(path);
    }
}

pub fn missing_paths<'a>(
    known: impl Iterator<Item = &'a String>,
    found: &[String],
    complete_roots: &[String],
) -> Vec<String> {
    let found: HashSet<&String> = found.iter().collect();
    known
        .filter(|path| {
            !found.contains(path) && complete_roots.iter().any(|root| within(path, root))
        })
        .cloned()
        .collect()
}

fn within(path: &str, root: &str) -> bool {
    let path = path.replace('\\', "/");
    let root = root.replace('\\', "/");
    #[cfg(windows)]
    let (path, root) = (path.to_lowercase(), root.to_lowercase());
    let root = root.trim_end_matches('/');
    path.strip_prefix(root)
        .is_some_and(|tail| tail.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_scan_request_rejects_old_result_and_keeps_both_roots_pending() {
        let mut discovery = Discovery::default();
        discovery.request(vec!["recordings".into()]);
        let (old, _) = discovery.begin(true).unwrap();
        discovery.request(vec!["other".into()]);
        assert!(discovery.begin(true).is_none());
        assert!(!discovery.finish(old));
        let (current, roots) = discovery.begin(true).unwrap();
        assert_eq!(roots, ["other", "recordings"]);
        assert!(discovery.finish(current));
    }

    #[test]
    fn reset_excludes_removed_watch_roots_from_the_next_scan() {
        let mut discovery = Discovery::default();
        discovery.request(vec!["removed".into()]);
        let (old, _) = discovery.begin(true).unwrap();
        discovery.reset(vec!["remaining".into()]);
        discovery.request(vec!["remaining".into()]);
        assert!(!discovery.finish(old));
        assert_eq!(discovery.begin(true).unwrap().1, ["remaining"]);
    }

    #[test]
    fn changed_files_reject_old_probes_and_failed_probes_can_retry() {
        let mut discovery = Discovery::default();
        let old = discovery.probe("clip.mp4", 10, 20, false).unwrap();
        assert!(discovery.probe("clip.mp4", 10, 20, false).is_none());
        let changed = discovery.probe("clip.mp4", 11, 21, false).unwrap();
        assert!(!discovery.finish_probe("clip.mp4", old));
        assert!(discovery.finish_probe("clip.mp4", changed));
        assert!(discovery.probe("clip.mp4", 11, 21, false).is_some());
    }

    #[test]
    fn removed_then_recreated_path_cannot_accept_its_old_probe() {
        let mut discovery = Discovery::default();
        let old = discovery.probe("clip.mp4", 10, 20, false).unwrap();
        discovery.remove("clip.mp4");
        let new = discovery.probe("clip.mp4", 10, 20, false).unwrap();
        assert!(!discovery.finish_probe("clip.mp4", old));
        assert!(discovery.finish_probe("clip.mp4", new));
    }

    #[test]
    fn missing_files_are_removed_only_from_successfully_scanned_roots() {
        let known = [
            "clips/gone.mp4",
            "clips/kept.mp4",
            "clips-other/kept.mp4",
            "offline/kept.mp4",
        ]
        .map(String::from);
        assert_eq!(
            missing_paths(known.iter(), &["clips/kept.mp4".into()], &["clips".into()]),
            ["clips/gone.mp4"],
        );
    }
}
