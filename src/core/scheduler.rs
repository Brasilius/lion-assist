use std::time::{Duration, Instant};
/// Serial processing supplies backpressure; pace requests across repeated tasks.
pub struct Pacer {
    interval: Duration,
    last: Option<Instant>,
}
impl Pacer {
    pub fn new(seconds: u64) -> Self {
        Self {
            interval: Duration::from_secs(seconds),
            last: None,
        }
    }
    pub fn wait(&mut self) {
        if let Some(last) = self.last
            && let Some(remaining) = self.interval.checked_sub(last.elapsed())
        {
            std::thread::sleep(remaining);
        }
        self.last = Some(Instant::now());
    }
}
