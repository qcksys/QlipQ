use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use qlipq_core::media::MediaInfo;
use tokio::sync::oneshot;

use crate::libav::{self, Player, PlayerHandle, ScrubDecoder};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct Settings {
    pub path: String,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub is_hdr: bool,
    pub gamma: f64,
    pub max_height: i64,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Started(Option<PlayerHandle>),
    Frame(Option<(u32, u32, Vec<u8>, f64)>),
    Superseded,
}

#[derive(Debug, Clone)]
pub struct Event {
    session: u64,
    generation: u64,
    pub outcome: Outcome,
    pub decode_hw: Option<bool>,
    pub dimensions: Option<(u32, u32)>,
}

#[derive(Default)]
struct Media {
    player: Option<Player>,
    scrubber: Option<ScrubDecoder>,
}

impl Media {
    fn open_scrubber(&mut self, settings: &Settings) {
        if self.scrubber.is_none() {
            self.scrubber = ScrubDecoder::open(
                &settings.path,
                settings.width,
                settings.height,
                settings.is_hdr,
                settings.gamma,
                settings.max_height,
            );
        }
    }
}

type Work = Box<dyn FnOnce(&mut Media) -> Outcome + Send>;

enum Command {
    Probe(String, oneshot::Sender<Result<(MediaInfo, bool), String>>),
    Run(u64, Work, oneshot::Sender<Event>),
    Stop { reset: bool },
    Release(oneshot::Sender<()>),
}

/// Owns a serialized media worker. Initialization, frame extraction and decoder joins all run there.
pub struct Session {
    id: u64,
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    sender: mpsc::Sender<Command>,
    settings: Option<Settings>,
}

impl Session {
    pub fn new() -> Self {
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let generation = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let worker_generation = Arc::clone(&generation);
        let worker_closed = Arc::clone(&closed);
        std::thread::spawn(move || {
            run_worker(id, receiver, worker_generation, worker_closed);
        });
        Self {
            id,
            generation,
            closed,
            sender,
            settings: None,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn probe(
        &self,
        path: String,
    ) -> impl Future<Output = Result<(MediaInfo, bool), String>> + use<> {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(Command::Probe(path, sender));
        async move {
            receiver
                .await
                .unwrap_or_else(|_| Err("Preview worker stopped".into()))
        }
    }

    pub fn accepts(&self, event: &Event) -> bool {
        event.session == self.id
            && event.generation == self.generation.load(Ordering::Acquire)
            && !self.closed.load(Ordering::Acquire)
    }

    pub fn configure(&mut self, settings: Settings) {
        self.settings = Some(settings);
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.sender.send(Command::Stop { reset: true });
    }

    pub fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.sender.send(Command::Stop { reset: false });
    }

    /// Resolves only after pending initialization/extraction and all decoder shutdowns have finished.
    pub fn release(&mut self) -> oneshot::Receiver<()> {
        self.closed.store(true, Ordering::Release);
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(Command::Release(sender));
        receiver
    }

    pub fn frame_at(&mut self, sec: f64) -> impl Future<Output = Event> + use<> {
        let settings = self.settings.clone();
        self.submit(Box::new(move |media| {
            let Some(settings) = settings else {
                return Outcome::Frame(None);
            };
            media.player = None;
            media.open_scrubber(&settings);
            Outcome::Frame(media.scrubber.as_mut().and_then(|s| s.frame_at(sec)))
        }))
    }

    pub fn start(
        &mut self,
        sec: f64,
        tracks: Vec<(i64, f64)>,
    ) -> impl Future<Output = Event> + use<> {
        let settings = self.settings.clone();
        self.submit(Box::new(move |media| {
            media.player = None;
            let Some(settings) = settings else {
                return Outcome::Started(None);
            };
            media.open_scrubber(&settings);
            media.player = libav::start_player(
                &settings.path,
                sec,
                settings.width,
                settings.height,
                settings.fps,
                settings.is_hdr,
                tracks,
                settings.gamma,
                settings.max_height,
            );
            Outcome::Started(media.player.as_ref().map(Player::handle))
        }))
    }

    fn submit(&mut self, work: Work) -> impl Future<Output = Event> + use<> {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let session = self.id;
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(Command::Run(generation, work, sender));
        async move {
            receiver.await.unwrap_or(Event {
                session,
                generation,
                outcome: Outcome::Superseded,
                decode_hw: None,
                dimensions: None,
            })
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn run_worker(
    session: u64,
    receiver: mpsc::Receiver<Command>,
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
) {
    let mut media = Media::default();
    for command in receiver {
        match command {
            Command::Probe(path, reply) => {
                let result = if closed.load(Ordering::Acquire) {
                    Err("Preview closed".into())
                } else {
                    libav::probe(&path)
                };
                let _ = reply.send(result);
            }
            Command::Stop { reset } => {
                media.player = None;
                if reset {
                    media.scrubber = None;
                }
            }
            Command::Release(reply) => {
                drop(media);
                let _ = reply.send(());
                return;
            }
            Command::Run(request, work, reply) => {
                let current = || {
                    request == generation.load(Ordering::Acquire) && !closed.load(Ordering::Acquire)
                };
                let outcome = if current() {
                    work(&mut media)
                } else {
                    Outcome::Superseded
                };
                let outcome = if current() {
                    outcome
                } else {
                    media.player = None;
                    Outcome::Superseded
                };
                let _ = reply.send(Event {
                    session,
                    generation: request,
                    outcome,
                    decode_hw: media.scrubber.as_ref().map(ScrubDecoder::hw_decode),
                    dimensions: media.scrubber.as_ref().map(ScrubDecoder::preview_dims),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_initialization_does_not_block_ui_and_release_waits_for_it() {
        let mut session = Session::new();
        let (entered, started) = mpsc::channel();
        let (finish, wait) = mpsc::channel();
        let _pending = session.submit(Box::new(move |_| {
            entered.send(std::thread::current().id()).unwrap();
            wait.recv().unwrap();
            Outcome::Started(None)
        }));
        assert_ne!(started.recv().unwrap(), std::thread::current().id());
        let mut released = session.release();
        assert!(matches!(
            released.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        finish.send(()).unwrap();
        released.blocking_recv().unwrap();
    }

    #[test]
    fn superseded_initialization_is_discarded_and_queued_starts_are_serialized() {
        let mut session = Session::new();
        let (entered, started) = mpsc::channel();
        let (finish, wait) = mpsc::channel();
        let first = session.submit(Box::new(move |_| {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Outcome::Started(None)
        }));
        started.recv().unwrap();
        let skipped = Arc::new(AtomicBool::new(false));
        let skipped_job = Arc::clone(&skipped);
        let second = session.submit(Box::new(move |_| {
            skipped_job.store(true, Ordering::Release);
            Outcome::Started(None)
        }));
        let third = session.submit(Box::new(|_| Outcome::Started(None)));
        finish.send(()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let first = runtime.block_on(first);
        let second = runtime.block_on(second);
        let third = runtime.block_on(third);
        assert!(matches!(first.outcome, Outcome::Superseded));
        assert!(matches!(second.outcome, Outcome::Superseded));
        assert!(!skipped.load(Ordering::Acquire));
        assert!(!session.accepts(&first));
        assert!(session.accepts(&third));
        session.release().blocking_recv().unwrap();
    }

    #[test]
    fn same_clip_new_session_rejects_old_results() {
        let mut old = Session::new();
        let result = old.submit(Box::new(|_| Outcome::Frame(None)));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime.block_on(result);
        let mut current = Session::new();
        assert!(!current.accepts(&result));
        old.release().blocking_recv().unwrap();
        current.release().blocking_recv().unwrap();
    }
}
