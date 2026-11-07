//! Background DDC thread. DDC/CI writes take ~50 ms each, so the UI never
//! talks to monitors directly: it updates a shared cache optimistically and
//! queues commands here. Rapid writes to the same (monitor, code) are
//! coalesced so dragging a slider sends only the latest value.

use crate::backend::{Backend, MonitorInfo};
use crate::vcp;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

pub type Key = (String, u8);

#[derive(Default, Debug)]
pub struct Shared {
    pub monitors: Vec<MonitorInfo>,
    /// Cached `(current, max)` for every feature that answered a read.
    pub values: HashMap<Key, (u32, u32)>,
    /// Bumped whenever the monitor list is re-enumerated.
    pub generation: u64,
    /// True once the first full read finished.
    pub ready: bool,
    pub last_error: Option<String>,
    /// Results of explicit `Worker::read` calls (raw VCP codes from the panel).
    pub adhoc: HashMap<Key, Result<(u32, u32), String>>,
    /// Keys with a queued write: reads must not clobber the optimistic value.
    dirty: HashMap<Key, u64>,
}

impl Shared {
    pub fn value(&self, id: &str, code: u8) -> Option<(u32, u32)> {
        self.values.get(&(id.to_string(), code)).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Monitors,
    Values,
    Error,
}

enum Cmd {
    Set(Key, u32, u64),
    Refresh { enumerate: bool },
    Read(Key),
    Ping(mpsc::Sender<()>),
    Stop,
}

pub struct Worker {
    tx: mpsc::Sender<Cmd>,
    pub shared: Arc<Mutex<Shared>>,
    seq: AtomicU64,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(backend: Box<dyn Backend>, notify: impl Fn(Event) + Send + 'static) -> Worker {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel();
        let s2 = shared.clone();
        let handle = std::thread::Builder::new()
            .name("glint-ddc".into())
            .spawn(move || run(backend, rx, s2, notify))
            .expect("spawn ddc thread");
        let w = Worker {
            tx,
            shared,
            seq: AtomicU64::new(0),
            handle: Some(handle),
        };
        w.refresh(true);
        w
    }

    pub fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue a write; the cache reflects the new value immediately.
    pub fn set(&self, id: &str, code: u8, value: u32) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let key = (id.to_string(), code);
        {
            let mut s = self.lock();
            let max = s.values.get(&key).map(|v| v.1).unwrap_or(100);
            s.values.insert(key.clone(), (value, max));
            s.dirty.insert(key.clone(), seq);
        }
        let _ = self.tx.send(Cmd::Set(key, value, seq));
    }

    /// Re-read all values (and re-enumerate monitors if `enumerate`).
    pub fn refresh(&self, enumerate: bool) {
        let _ = self.tx.send(Cmd::Refresh { enumerate });
    }

    /// Read any VCP code; the result lands in `Shared::adhoc`.
    pub fn read(&self, id: &str, code: u8) {
        let _ = self.tx.send(Cmd::Read((id.to_string(), code)));
    }

    /// Block until everything queued so far has been processed.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Cmd::Ping(tx)).is_err() {
            return false;
        }
        rx.recv_timeout(timeout).is_ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn lock(s: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(
    mut backend: Box<dyn Backend>,
    rx: mpsc::Receiver<Cmd>,
    shared: Arc<Mutex<Shared>>,
    notify: impl Fn(Event),
) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        batch.extend(rx.try_iter());

        let (mut enumerate, mut refresh) = (false, false);
        let mut sets: Vec<(Key, u32, u64)> = Vec::new();
        let mut pings = Vec::new();
        let mut reads: Vec<Key> = Vec::new();
        for cmd in batch {
            match cmd {
                Cmd::Stop => return,
                Cmd::Refresh { enumerate: e } => {
                    refresh = true;
                    enumerate |= e;
                }
                // Keep first position (preserves profile ordering), latest value.
                Cmd::Set(k, v, seq) => match sets.iter_mut().find(|s| s.0 == k) {
                    Some(slot) => {
                        slot.1 = v;
                        slot.2 = seq;
                    }
                    None => sets.push((k, v, seq)),
                },
                Cmd::Ping(tx) => pings.push(tx),
                Cmd::Read(k) => reads.push(k),
            }
        }

        if enumerate {
            match backend.enumerate() {
                Ok(mons) => {
                    let mut s = lock(&shared);
                    s.values.retain(|k, _| mons.iter().any(|m| m.id == k.0));
                    s.monitors = mons;
                    s.generation += 1;
                }
                Err(e) => {
                    lock(&shared).last_error = Some(format!("enumerate: {e}"));
                    notify(Event::Error);
                }
            }
            notify(Event::Monitors);
        }

        if !sets.is_empty() {
            let mut failed = Vec::new();
            for (key, value, seq) in sets {
                let res = backend.set(&key.0, key.1, value);
                let settle = vcp::settle_ms(key.1);
                if settle > 0 && res.is_ok() {
                    std::thread::sleep(Duration::from_millis(settle));
                }
                let mut s = lock(&shared);
                if s.dirty.get(&key) == Some(&seq) {
                    s.dirty.remove(&key);
                }
                if let Err(e) = res {
                    s.last_error = Some(format!(
                        "set {} 0x{:02X}={value}: {e}",
                        short(&key.0),
                        key.1
                    ));
                    failed.push(key);
                }
            }
            // Resync failed writes with the monitor's real value.
            for key in &failed {
                let r = backend.get(&key.0, key.1);
                store(&shared, key, r);
            }
            if !failed.is_empty() {
                notify(Event::Error);
            }
            notify(Event::Values);
        }

        if !reads.is_empty() {
            for key in reads {
                let r = backend.get(&key.0, key.1);
                if let Ok(v) = r
                    && vcp::by_code(key.1).is_some()
                {
                    store(&shared, &key, Ok(v));
                }
                lock(&shared).adhoc.insert(key, r);
            }
            notify(Event::Values);
        }

        if refresh {
            let monitors = lock(&shared).monitors.clone();
            for m in &monitors {
                for f in vcp::FEATURES {
                    let key = (m.id.clone(), f.code);
                    let r = backend.get(&m.id, f.code);
                    store(&shared, &key, r);
                }
                notify(Event::Values);
            }
            lock(&shared).ready = true;
            notify(Event::Values);
        }

        for p in pings {
            let _ = p.send(());
        }
    }
}

fn store(shared: &Mutex<Shared>, key: &Key, r: Result<(u32, u32), String>) {
    let mut s = lock(shared);
    if s.dirty.contains_key(key) {
        return;
    }
    match r {
        Ok(v) => {
            s.values.insert(key.clone(), v);
        }
        Err(_) => {
            s.values.remove(key);
        }
    }
}

fn short(id: &str) -> String {
    MonitorInfo {
        id: id.to_string(),
        ..Default::default()
    }
    .short_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use std::sync::atomic::AtomicUsize;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn initial_refresh_populates_cache() {
        let events = Arc::new(AtomicUsize::new(0));
        let e2 = events.clone();
        let w = Worker::spawn(Box::new(MockBackend::two_aoc()), move |_| {
            e2.fetch_add(1, Ordering::SeqCst);
        });
        assert!(w.wait_idle(T));
        let s = w.lock();
        assert!(s.ready);
        assert_eq!(s.generation, 1);
        assert_eq!(s.monitors.len(), 2);
        let id0 = s.monitors[0].id.clone();
        assert_eq!(s.value(&id0, vcp::BRIGHTNESS), Some((19, 100)));
        assert_eq!(
            s.value(&id0, vcp::SHARPNESS),
            Some((50, 100)),
            "unadvertised but answering"
        );
        assert!(events.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn rapid_sets_are_coalesced_and_final_value_wins() {
        let mut b = MockBackend::two_aoc();
        b.delay = Duration::from_millis(25);
        let writes = b.writes.clone();
        let id = b.monitors[0].id.clone();
        let w = Worker::spawn(Box::new(b), |_| {});
        assert!(w.wait_idle(T));
        for v in 0..=60 {
            w.set(&id, vcp::BRIGHTNESS, v);
            std::thread::sleep(Duration::from_millis(2));
        }
        // optimistic cache is immediate
        assert_eq!(w.lock().value(&id, vcp::BRIGHTNESS), Some((60, 100)));
        assert!(w.wait_idle(T));
        let writes = writes.lock().unwrap();
        let bright: Vec<_> = writes.iter().filter(|w| w.1 == vcp::BRIGHTNESS).collect();
        assert!(
            bright.len() < 30,
            "expected coalescing, got {} writes",
            bright.len()
        );
        assert_eq!(bright.last().unwrap().2, 60);
        assert_eq!(w.lock().value(&id, vcp::BRIGHTNESS), Some((60, 100)));
    }

    #[test]
    fn coalescing_keeps_first_seen_order() {
        let mut b = MockBackend::two_aoc();
        b.delay = Duration::from_millis(40);
        let writes = b.writes.clone();
        let id = b.monitors[0].id.clone();
        let w = Worker::spawn(Box::new(b), |_| {});
        assert!(w.wait_idle(T));
        // Occupy the thread so the next three land in one batch.
        w.set(&id, vcp::CONTRAST, 1);
        std::thread::sleep(Duration::from_millis(10));
        w.set(&id, vcp::COLOR_PRESET, 11);
        w.set(&id, vcp::RED, 40);
        w.set(&id, vcp::COLOR_PRESET, 11);
        assert!(w.wait_idle(T));
        let codes: Vec<u8> = writes.lock().unwrap().iter().map(|w| w.1).collect();
        assert_eq!(codes, vec![vcp::CONTRAST, vcp::COLOR_PRESET, vcp::RED]);
    }

    #[test]
    fn failed_set_resyncs_real_value_and_reports() {
        let mut b = MockBackend::two_aoc();
        b.fail_set = vec![vcp::INPUT];
        let id = b.monitors[1].id.clone();
        let errors = Arc::new(AtomicUsize::new(0));
        let e2 = errors.clone();
        let w = Worker::spawn(Box::new(b), move |e| {
            if e == Event::Error {
                e2.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert!(w.wait_idle(T));
        w.set(&id, vcp::INPUT, 0x0F);
        assert!(w.wait_idle(T));
        assert_eq!(w.lock().value(&id, vcp::INPUT), Some((0x11, 100)));
        assert!(w.lock().last_error.as_deref().unwrap().contains("0x60"));
        assert_eq!(errors.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn refresh_does_not_clobber_pending_write() {
        let mut b = MockBackend::two_aoc();
        b.delay = Duration::from_millis(30);
        let id = b.monitors[0].id.clone();
        let w = Worker::spawn(Box::new(b), |_| {});
        assert!(w.wait_idle(T));
        w.set(&id, vcp::CONTRAST, 1); // occupies the thread
        std::thread::sleep(Duration::from_millis(5));
        w.refresh(false);
        w.set(&id, vcp::BRIGHTNESS, 77);
        assert_eq!(w.lock().value(&id, vcp::BRIGHTNESS), Some((77, 100)));
        assert!(w.wait_idle(T));
        assert_eq!(w.lock().value(&id, vcp::BRIGHTNESS), Some((77, 100)));
    }

    #[test]
    fn adhoc_reads() {
        let b = MockBackend::two_aoc();
        let id = b.monitors[0].id.clone();
        let w = Worker::spawn(Box::new(b), |_| {});
        w.read(&id, vcp::SHARPNESS);
        w.read(&id, 0xAC);
        assert!(w.wait_idle(T));
        let s = w.lock();
        assert_eq!(s.adhoc[&(id.clone(), vcp::SHARPNESS)], Ok((50, 100)));
        assert!(s.adhoc[&(id, 0xAC)].is_err());
    }

    #[test]
    fn drop_stops_thread() {
        let w = Worker::spawn(Box::new(MockBackend::two_aoc()), |_| {});
        drop(w); // must not hang
    }
}
