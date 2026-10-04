use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use iced::widget::image::Handle;
use qlipq_core::media::MediaInfo;

use crate::{libav, video};

static THUMBNAIL_SEM: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
pub const WIDTH: f32 = 112.0;
pub const HEIGHT: f32 = 63.0;

#[derive(Clone)]
pub struct Source {
    pub path: String,
    pub media: MediaInfo,
    pub is_hdr: bool,
    pub gamma: f64,
}

impl Source {
    fn max_height(&self) -> i64 {
        (224.0 * self.media.height as f64 / self.media.width.max(1) as f64)
            .min(126.0)
            .max(2.0) as i64
    }

    fn thumbnail(&self) -> Option<Handle> {
        let mut decoder = libav::ScrubDecoder::open(
            &self.path,
            self.media.width,
            self.media.height,
            self.is_hdr,
            self.gamma,
            self.max_height(),
        )?;
        let at = (self.media.duration_sec * 0.1).clamp(0.0, 5.0);
        let (w, h, data, _) = decoder.frame_at(at).or_else(|| decoder.frame_at(0.0))?;
        Some(Handle::from_rgba(w, h, data))
    }
}

pub async fn thumbnail(source: Source) -> Option<Handle> {
    let _permit = THUMBNAIL_SEM.acquire().await.ok()?;
    crate::blocking(move || source.thumbnail()).await
}

// File mutations wait for thumbnail decoders to release their Windows file handles.
pub async fn file_operation<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let _permits = THUMBNAIL_SEM.acquire_many(2).await.unwrap();
    crate::blocking(f).await
}

pub struct HoverPreview {
    pub id: String,
    pub frame: video::SharedFrame,
    pub aspect: f32,
    ready: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl HoverPreview {
    pub fn start(id: String, source: Source) -> Self {
        let frame = video::new_shared_frame();
        let ready = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let target = frame.clone();
        let loaded = ready.clone();
        let stop = cancel.clone();
        let aspect = source.media.width.max(1) as f32 / source.media.height.max(1) as f32;
        let worker = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some(player) = libav::start_player(
                    &source.path,
                    0.0,
                    source.media.width,
                    source.media.height,
                    source.media.fps,
                    source.is_hdr,
                    Vec::new(),
                    source.gamma,
                    source.max_height(),
                ) else {
                    return;
                };
                let (w, h) = player.dimensions();
                let mut played = false;
                while !stop.load(Ordering::Relaxed) {
                    match player.poll() {
                        libav::FramePoll::Frame(data) => {
                            video::push_frame(&target, w, h, data);
                            loaded.store(true, Ordering::Relaxed);
                            played = true;
                        }
                        libav::FramePoll::Ended => break,
                        libav::FramePoll::Empty => {}
                    }
                    std::thread::park_timeout(Duration::from_millis(16));
                }
                if !played {
                    break;
                }
            }
        });
        Self {
            id,
            frame,
            aspect,
            ready,
            cancel,
            worker: Some(worker),
        }
    }

    pub fn has_frame(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
}

impl Drop for HoverPreview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_thumbnail_and_muted_hover_then_releases_file() {
        let path =
            std::env::temp_dir().join(format!("qlipq-queue-preview-{}.y4m", std::process::id()));
        let mut data = b"YUV4MPEG2 W64 H36 F10:1 Ip A1:1 C420jpeg\n".to_vec();
        for n in 0..10 {
            data.extend_from_slice(b"FRAME\n");
            data.extend(std::iter::repeat_n(32 + n * 10, 64 * 36));
            data.extend(std::iter::repeat_n(128, 64 * 36 / 2));
        }
        std::fs::write(&path, data).unwrap();
        let (media, is_hdr) = libav::probe(path.to_str().unwrap()).unwrap();
        let source = Source {
            path: path.to_string_lossy().into_owned(),
            media,
            is_hdr,
            gamma: 1.0,
        };
        assert!(source.thumbnail().is_some());
        let preview = HoverPreview::start("test".into(), source);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !preview.has_frame() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(preview.has_frame());
        let mut previous = video::frame_sample(&preview.frame).1;
        let mut advanced = false;
        let mut looped = false;
        while std::time::Instant::now() < deadline {
            let pixel = video::frame_sample(&preview.frame).1;
            advanced |= pixel > previous;
            if advanced && pixel < previous {
                looped = true;
                break;
            }
            previous = pixel;
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(looped, "hover preview must advance and restart at EOF");
        let frame = preview.frame.clone();
        drop(preview);
        let stopped = video::frame_sample(&frame);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(video::frame_sample(&frame), stopped);
        std::fs::remove_file(path).unwrap();
    }
}
