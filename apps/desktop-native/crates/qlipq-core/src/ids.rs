use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);
const BASE36: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Generate a short, URL/file-safe id for queue items (12 base36 chars). Not cryptographic;
/// combines the wall-clock and a process-local counter so concurrent calls don't collide.
pub fn create_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut n = nanos ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut out = String::with_capacity(12);
    for _ in 0..12 {
        out.push(BASE36[(n % 36) as usize] as char);
        n /= 36;
        if n == 0 {
            n = counter.wrapping_add(1);
        }
    }
    out
}
