use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::{OwnedRwLockReadGuard, RwLock};

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
