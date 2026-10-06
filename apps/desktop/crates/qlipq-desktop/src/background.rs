use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::{OwnedRwLockReadGuard, RwLock};

pub fn save(f: impl FnOnce() + Send + 'static) -> iced::Task<()> {
    type Write = Box<dyn FnOnce() + Send>;
    static WRITER: OnceLock<std::sync::mpsc::Sender<Write>> = OnceLock::new();
    let writer = WRITER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<Write>();
        std::thread::spawn(move || {
            for write in receive {
                write();
            }
        });
        send
    });
    let (send, receive) = tokio::sync::oneshot::channel();
    writer
        .send(Box::new(move || {
            f();
            let _ = send.send(());
        }))
        .unwrap();
    iced::Task::perform(
        async move {
            let _ = receive.await;
        },
        |_| (),
    )
}

fn file_gate(path: &str) -> Arc<RwLock<()>> {
    static FILES: OnceLock<Mutex<HashMap<String, Weak<RwLock<()>>>>> = OnceLock::new();
    let mut files = FILES.get_or_init(Mutex::default).lock().unwrap();
    files.retain(|_, gate| gate.strong_count() > 0);
    let key = if cfg!(windows) {
        path.replace('\\', "/").to_lowercase()
    } else {
        path.to_owned()
    };
    if let Some(gate) = files.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(RwLock::new(()));
    files.insert(key, Arc::downgrade(&gate));
    gate
}

pub fn read_media(path: &str) -> OwnedRwLockReadGuard<()> {
    assert_worker();
    iced::futures::executor::block_on(file_gate(path).read_owned())
}

pub fn write_media(path: &str) -> tokio::sync::OwnedRwLockWriteGuard<()> {
    assert_worker();
    iced::futures::executor::block_on(file_gate(path).write_owned())
}

/// Native decoder destruction may wait for GPU work or join decode threads.
pub struct Media<T: Send + 'static>(Option<(T, OwnedRwLockReadGuard<()>)>);

impl<T: Send + 'static> Media<T> {
    pub fn open(path: &str, open: impl FnOnce() -> Option<T>) -> Option<Self> {
        let lease = read_media(path);
        open().map(|value| Self(Some((value, lease))))
    }
}

impl<T: Send + 'static> Deref for Media<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0.as_ref().unwrap().0
    }
}

impl<T: Send + 'static> DerefMut for Media<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0.as_mut().unwrap().0
    }
}

impl<T: Send + 'static> Drop for Media<T> {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            std::thread::spawn(move || drop(value));
        }
    }
}

/// Messages are cloneable; ownership of a native resource must only be claimed once.
pub struct Delivery<T>(Arc<Mutex<Option<T>>>);

#[cfg(test)]
pub static UI_THREADS: Mutex<Vec<std::thread::ThreadId>> = Mutex::new(Vec::new());

#[cfg(test)]
pub struct UiThread;

#[cfg(test)]
impl UiThread {
    pub fn enter() -> Self {
        UI_THREADS.lock().unwrap().push(std::thread::current().id());
        Self
    }
}

#[cfg(test)]
impl Drop for UiThread {
    fn drop(&mut self) {
        let mut threads = UI_THREADS.lock().unwrap();
        let index = threads
            .iter()
            .rposition(|id| *id == std::thread::current().id())
            .unwrap();
        threads.remove(index);
    }
}

pub fn assert_worker() {
    #[cfg(test)]
    assert!(
        !UI_THREADS
            .lock()
            .unwrap()
            .contains(&std::thread::current().id()),
        "Blocking work ran on the UI thread"
    );
}

impl<T> Delivery<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap().take()
    }
}

impl<T> Clone for Delivery<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> std::fmt::Debug for Delivery<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Delivery")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct SlowDecoder(mpsc::Sender<()>, mpsc::Receiver<()>);

    impl Drop for SlowDecoder {
        fn drop(&mut self) {
            assert_worker();
            self.0.send(()).unwrap();
            self.1.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }

    #[test]
    fn decoder_shutdown_never_blocks_ui_and_file_mutation_waits_for_handles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("recording.mkv")
            .to_string_lossy()
            .into_owned();
        let (entered, waiting) = mpsc::channel();
        let (resume, blocked) = mpsc::channel();
        let media = Media::open(&path, || Some(SlowDecoder(entered, blocked))).unwrap();
        let started = Instant::now();
        {
            let _ui = UiThread::enter();
            drop(media);
        }
        assert!(started.elapsed() < Duration::from_millis(100));
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let (mutated, finished) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _exclusive = write_media(&path);
            mutated.send(()).unwrap();
        });
        assert!(finished.recv_timeout(Duration::from_millis(50)).is_err());
        resume.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn persistence_preserves_order_without_waiting_on_ui() {
        let (resume, blocked) = mpsc::channel();
        let (send, receive) = mpsc::channel();
        let first = send.clone();
        {
            let _ui = UiThread::enter();
            drop(save(move || {
                assert_worker();
                blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                first.send(1).unwrap();
            }));
            drop(save(move || {
                assert_worker();
                send.send(2).unwrap();
            }));
        }
        assert!(receive.recv_timeout(Duration::from_millis(50)).is_err());
        resume.send(()).unwrap();
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
    }
}
