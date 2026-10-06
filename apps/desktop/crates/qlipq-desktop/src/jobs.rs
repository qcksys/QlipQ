use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use qlipq_core::config::{AfterExportSettings, OutputSettings};
use qlipq_core::edit_spec::EditSpec;
use qlipq_core::media::MediaInfo;

#[derive(Debug, Clone)]
pub struct AfterExport {
    pub item_id: String,
    pub input: String,
    pub settings: AfterExportSettings,
}

#[derive(Debug, Clone)]
pub struct ExportRequest {
    pub source: AfterExport,
    pub output_path: String,
    pub spec: EditSpec,
    pub output: OutputSettings,
    pub media: MediaInfo,
    pub is_hdr: bool,
    pub metadata: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct ExportJob {
    pub id: u64,
    pub request: ExportRequest,
    pub progress: Arc<Mutex<f32>>,
    pub cancel: Arc<AtomicBool>,
}

impl ExportJob {
    pub fn progress(&self) -> f32 {
        self.progress.try_lock().map(|p| *p).unwrap_or(0.0)
    }
}

#[derive(Default)]
pub struct Exports {
    active: Option<ExportJob>,
    next_id: u64,
}

impl Exports {
    pub fn active(&self) -> Option<&ExportJob> {
        self.active.as_ref()
    }

    pub fn contains(&self, item_id: &str) -> bool {
        self.active
            .as_ref()
            .is_some_and(|job| job.request.source.item_id == item_id)
    }

    pub fn start(&mut self, request: ExportRequest) -> Option<ExportJob> {
        if self.active.is_some() {
            return None;
        }
        self.next_id += 1;
        let job = ExportJob {
            id: self.next_id,
            request,
            progress: Arc::new(Mutex::new(0.0)),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        self.active = Some(job.clone());
        Some(job)
    }

    pub fn finish(&mut self, id: u64) -> Option<ExportJob> {
        if self.active.as_ref().is_some_and(|job| job.id == id) {
            self.active.take()
        } else {
            None
        }
    }

    pub fn cancel(&self) {
        if let Some(job) = &self.active {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Exports {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Debug, Clone)]
pub enum FileMutation {
    Delete,
    Rename(String),
}

#[derive(Debug, Clone)]
pub struct FileOperation {
    pub item_id: String,
    pub path: String,
    pub mutation: FileMutation,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str) -> ExportRequest {
        ExportRequest {
            source: AfterExport {
                item_id: id.into(),
                input: format!("{id}.mkv"),
                settings: AfterExportSettings::default(),
            },
            output_path: format!("out/{id}.mp4"),
            spec: qlipq_core::edit_spec::default_edit_spec(None),
            output: OutputSettings::default(),
            media: MediaInfo {
                duration_sec: 1.0,
                width: 16,
                height: 16,
                video_codec: "h264".into(),
                fps: 30.0,
                audio_streams: vec![],
                size_bytes: None,
                encoder: None,
            },
            is_hdr: false,
            metadata: vec![],
        }
    }

    #[test]
    fn concurrent_start_cannot_replace_destination_or_cancel_handle() {
        let mut exports = Exports::default();
        let first = exports.start(request("a")).unwrap();
        assert!(exports.start(request("b")).is_none());
        exports.cancel();
        assert!(first.cancel.load(Ordering::Relaxed));
        let finished = exports.finish(first.id).unwrap();
        assert_eq!(finished.request.source.item_id, "a");
        assert_eq!(finished.request.output_path, "out/a.mp4");
    }

    #[test]
    fn late_completion_cannot_finish_a_new_export() {
        let mut exports = Exports::default();
        let first = exports.start(request("a")).unwrap();
        exports.finish(first.id).unwrap();
        let second = exports.start(request("a")).unwrap();
        assert!(exports.finish(first.id).is_none());
        assert_eq!(exports.active().unwrap().id, second.id);
    }
}
