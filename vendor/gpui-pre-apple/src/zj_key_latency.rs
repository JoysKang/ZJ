// ZJ patch: per-window keyDown → next successful Metal commit return.
// CPU submission only: neither GPU completion nor compositor / display scan-out.
use std::{sync::OnceLock, time::Instant};

const MAX_PENDING: usize = 256;

#[derive(Default)]
pub struct KeyLatency {
    pending: Vec<Instant>,
}

impl KeyLatency {
    pub fn key_down(&mut self) {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        if *ENABLED.get_or_init(|| std::env::var_os("ZJ_LATENCY_LOG").is_some_and(|v| v == "1"))
            && !self.record(Instant::now())
        {
            eprintln!("event=key_latency_dropped reason=pending_full");
        }
    }

    fn record(&mut self, pressed: Instant) -> bool {
        if self.pending.len() == MAX_PENDING {
            return false;
        }
        self.pending.push(pressed);
        true
    }

    // Call immediately after commit(), before waiting for scheduling or presenting.
    // Skipped / failed draws must not call this or consume pending keys.
    pub fn submitted(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let submitted = Instant::now();
        for us in self.drain(submitted) {
            eprintln!("event=key_cpu_submit us={us} start=native_key_down end=metal_commit_return");
        }
    }

    fn drain(&mut self, submitted: Instant) -> impl Iterator<Item = u128> + '_ {
        self.pending
            .drain(..)
            .map(move |pressed| submitted.duration_since(pressed).as_micros())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_submission_consumes_keys_and_coalesced_keys_share_endpoint() {
        let start = Instant::now();
        let mut latency = KeyLatency::default();
        assert!(latency.record(start));
        assert!(latency.record(start + Duration::from_micros(100)));
        // A pre-draw callback or a failed draw does not drain the queue.
        assert_eq!(latency.pending.len(), 2);
        assert_eq!(
            latency
                .drain(start + Duration::from_micros(500))
                .collect::<Vec<_>>(),
            [500, 400]
        );
        assert_eq!(latency.drain(start + Duration::from_micros(600)).count(), 0);
    }

    #[test]
    fn windows_do_not_consume_each_others_keys() {
        let start = Instant::now();
        let mut first = KeyLatency::default();
        let mut second = KeyLatency::default();
        first.record(start);
        assert_eq!(second.drain(start).count(), 0);
        assert_eq!(first.drain(start).collect::<Vec<_>>(), [0]);
    }

    #[test]
    fn pending_keys_are_bounded_without_overwriting_earlier_samples() {
        let start = Instant::now();
        let mut latency = KeyLatency::default();
        for _ in 0..MAX_PENDING {
            assert!(latency.record(start));
        }
        assert!(!latency.record(start));
        assert_eq!(latency.drain(start).count(), MAX_PENDING);
        assert!(latency.record(start));
    }
}
