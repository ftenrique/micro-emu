//! Short-lived memo for "does this process belong to agent X" checks.
//!
//! The desktop-liveness probes enumerate every top-level window and resolve
//! each owner's image path (OpenProcess + QueryFullProcessImageName). Doing
//! that for a few hundred windows per agent every couple of seconds is
//! measurable, and a process's image never changes, so remember the answer
//! briefly. The TTL bounds the damage from PID reuse.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(5);

static CACHE: Mutex<Option<HashMap<(&'static str, u32), (Instant, bool)>>> = Mutex::new(None);

pub fn cached(agent: &'static str, process_id: u32, check: impl FnOnce(u32) -> bool) -> bool {
    let now = Instant::now();
    if let Ok(mut guard) = CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(&(at, value)) = map.get(&(agent, process_id)) {
            if now.duration_since(at) < TTL {
                return value;
            }
        }
        if map.len() > 4096 {
            map.retain(|_, (at, _)| now.duration_since(*at) < TTL);
        }
        drop(guard);
    }
    let value = check(process_id);
    if let Ok(mut guard) = CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert((agent, process_id), (now, value));
    }
    value
}
