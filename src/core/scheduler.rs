use std::time::{Duration, Instant};
/// Serial processing supplies backpressure; pace requests across repeated tasks.
pub struct Pacer {
    interval: Duration,
    last: Option<Instant>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
impl Pacer {
    pub fn new(seconds: u64) -> Self {
        Self {
            interval: Duration::from_secs(seconds),
            last: None,
            cancel: None,
        }
    }
    pub fn set_cancel(&mut self, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.cancel = Some(cancel);
    }
    pub fn wait(&mut self) {
        if let Some(last) = self.last
            && let Some(remaining) = self.interval.checked_sub(last.elapsed())
        {
            let start = Instant::now();
            while start.elapsed() < remaining {
                if self
                    .cancel
                    .as_ref()
                    .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
                {
                    return;
                }
                std::thread::sleep(
                    remaining
                        .saturating_sub(start.elapsed())
                        .min(Duration::from_millis(100)),
                );
            }
        }
        self.last = Some(Instant::now());
    }
}
