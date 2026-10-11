//! Per-source token bucket plus concurrency cap (`SKILL.md` §5). After a 429 the refill
//! rate is halved for 10 minutes.

use std::time::Duration;

use tokio::sync::{AcquireError, Mutex, Semaphore, SemaphorePermit};
use tokio::time::Instant;

use super::SourceSpec;

/// How long a 429 halves the refill rate.
const SLOW_FOR: Duration = Duration::from_secs(600);

struct State {
    tokens: f64,
    last: Instant,
    slow_until: Option<Instant>,
}

/// One source's bucket.
pub struct Bucket {
    burst: f64,
    rate: f64,
    state: Mutex<State>,
    in_flight: Semaphore,
}

impl Bucket {
    /// A full bucket for `spec`.
    #[must_use]
    pub fn new(spec: &SourceSpec) -> Self {
        Self {
            burst: spec.burst,
            rate: spec.rate,
            state: Mutex::new(State {
                tokens: spec.burst,
                last: Instant::now(),
                slow_until: None,
            }),
            in_flight: Semaphore::new(spec.concurrency.max(1)),
        }
    }

    /// Wait for a concurrency slot and a token. Hold the permit for the request's duration.
    ///
    /// Cancellation-safe: dropping the future before it resolves takes no token.
    ///
    /// # Errors
    ///
    /// Only if the semaphore was closed, which this type never does.
    pub async fn acquire(&self) -> Result<SemaphorePermit<'_>, AcquireError> {
        let permit = self.in_flight.acquire().await?;
        loop {
            let wait = {
                let mut s = self.state.lock().await;
                let now = Instant::now();
                let rate = match s.slow_until {
                    Some(t) if t > now => self.rate / 2.0,
                    _ => self.rate,
                };
                s.tokens =
                    (s.tokens + now.duration_since(s.last).as_secs_f64() * rate).min(self.burst);
                s.last = now;
                if s.tokens >= 1.0 {
                    s.tokens -= 1.0;
                    break;
                }
                Duration::from_secs_f64((1.0 - s.tokens) / rate)
            };
            tokio::time::sleep(wait).await;
        }
        Ok(permit)
    }

    /// A 429 was seen: empty the bucket and halve the rate for 10 minutes.
    pub async fn throttled(&self) {
        let mut s = self.state.lock().await;
        s.tokens = 0.0;
        s.slow_until = Some(Instant::now() + SLOW_FOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn burst_then_rate_then_half_rate_after_429() {
        let spec = SourceSpec {
            burst: 2.0,
            rate: 4.0,
            concurrency: 8,
            ..SourceSpec::EODHD
        };
        let b = Bucket::new(&spec);
        let t0 = Instant::now();
        for _ in 0..2 {
            drop(b.acquire().await.ok());
        }
        assert!(t0.elapsed() < Duration::from_millis(1), "the burst is free");
        drop(b.acquire().await.ok());
        assert_eq!(
            t0.elapsed(),
            Duration::from_millis(250),
            "then one token per 1/rate"
        );
        b.throttled().await;
        let t1 = Instant::now();
        drop(b.acquire().await.ok());
        assert_eq!(
            t1.elapsed(),
            Duration::from_millis(500),
            "half rate after a 429"
        );
    }
}
