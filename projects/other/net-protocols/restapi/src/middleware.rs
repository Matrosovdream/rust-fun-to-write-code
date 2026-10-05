//! Two middlewares: an access logger and a per-IP token-bucket rate limiter.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::http::{Request, Response};
use crate::router::{Middleware, Next};

/// Logs `peer method path status bytes duration` for every request.
pub struct Logger;

impl Middleware for Logger {
    fn handle(&self, req: Request, next: Next<'_>) -> Response {
        let start = Instant::now();
        // `next.run` takes the request by value, so copy what we log first.
        let (peer, method, path) = (req.peer, req.method.clone(), req.path.clone());
        let resp = next.run(req);
        log(peer, &method, &path, &resp, start);
        resp
    }
}

/// Also used for requests that fail to parse and never reach the router.
pub fn log(peer: SocketAddr, method: &str, path: &str, resp: &Response, start: Instant) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "{peer} {method} {path} {} {}B {ms:.1}ms",
        resp.status,
        resp.body.len()
    );
}

/// A bucket holds up to `capacity` tokens and refills at `rate` tokens
/// per second. Each request takes one token, so a client can burst
/// `capacity` requests and then sustain `rate` per second.
///
/// Time is a parameter (`now`), not read from the clock inside. That makes
/// the bucket a pure state machine that tests can drive through any
/// timeline instantly.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    rate: f64,
    tokens: f64,
    updated: Instant,
}

impl TokenBucket {
    /// # Panics
    /// If `rate` is zero: the bucket would never refill.
    pub fn new(capacity: u32, rate: u32, now: Instant) -> TokenBucket {
        assert!(rate > 0, "refill rate must be positive");
        let capacity = f64::from(capacity);
        TokenBucket {
            capacity,
            rate: f64::from(rate),
            tokens: capacity,
            updated: now,
        }
    }

    /// Takes a token if one is available. Otherwise returns how long until
    /// one will be.
    pub fn try_take(&mut self, now: Instant) -> Result<(), Duration> {
        // Refill lazily for the time since the last call, instead of
        // running a timer. `saturating_` guards against `now` going back.
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.capacity);
        self.updated = self.updated.max(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / self.rate))
        }
    }
}

/// One bucket per client IP. Over the limit, it answers 429 Too Many
/// Requests with `Retry-After` and never calls the handler.
pub struct RateLimiter {
    capacity: u32,
    rate: u32,
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
    /// The injected clock: `Instant::now` in production, a fake in tests.
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
}

impl RateLimiter {
    pub fn new(capacity: u32, per_second: u32) -> RateLimiter {
        RateLimiter::with_clock(capacity, per_second, Instant::now)
    }

    pub fn with_clock(
        capacity: u32,
        per_second: u32,
        clock: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> RateLimiter {
        let buckets = Mutex::new(HashMap::new());
        RateLimiter {
            capacity,
            rate: per_second,
            buckets,
            clock: Box::new(clock),
        }
    }
}

impl Middleware for RateLimiter {
    fn handle(&self, req: Request, next: Next<'_>) -> Response {
        let now = (self.clock)();
        // The block scopes the MutexGuard: the lock is released before the
        // handler runs, so slow handlers don't serialize every client.
        let verdict = {
            let mut buckets = self.buckets.lock().expect("rate limiter lock poisoned");
            let bucket = buckets
                .entry(req.peer.ip())
                .or_insert_with(|| TokenBucket::new(self.capacity, self.rate, now));
            bucket.try_take(now)
        };
        match verdict {
            Ok(()) => next.run(req),
            Err(wait) => {
                // Retry-After is in whole seconds (RFC 9110 §10.2.3); round up.
                let secs = (wait.as_secs_f64().ceil() as u64).max(1);
                Response::error(429, "too many requests")
                    .with_header("Retry-After", &secs.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::Router;
    use std::sync::Arc;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn bucket_allows_a_burst_then_refills_at_the_rate() {
        let t0 = Instant::now();
        let mut bucket = TokenBucket::new(3, 2, t0); // burst 3, 2 per second
        for _ in 0..3 {
            assert_eq!(bucket.try_take(t0), Ok(()));
        }
        assert_eq!(bucket.try_take(t0), Err(ms(500)));
        assert_eq!(bucket.try_take(t0 + ms(250)), Err(ms(250)));
        assert_eq!(bucket.try_take(t0 + ms(500)), Ok(()));
        assert!(bucket.try_take(t0 + ms(500)).is_err());
    }

    #[test]
    fn bucket_never_holds_more_than_capacity() {
        let t0 = Instant::now();
        let mut bucket = TokenBucket::new(2, 1, t0);
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(bucket.try_take(later), Ok(()));
        assert_eq!(bucket.try_take(later), Ok(()));
        assert!(bucket.try_take(later).is_err());
    }

    #[test]
    fn limiter_answers_429_per_ip_until_time_passes() {
        // A fake clock we can move by hand. The limiter only sees a closure.
        let now = Arc::new(Mutex::new(Instant::now()));
        let clock = Arc::clone(&now);
        let limiter = RateLimiter::with_clock(2, 1, move || *clock.lock().unwrap());
        let router = Router::new()
            .get("/", |_: &Request| Response::new(200))
            .wrap(limiter);
        let from = |ip: [u8; 4]| {
            let mut req = Request::new("GET", "/");
            req.peer = SocketAddr::from((ip, 1234));
            req
        };

        assert_eq!(router.handle(from([1, 1, 1, 1])).status, 200);
        assert_eq!(router.handle(from([1, 1, 1, 1])).status, 200);
        let limited = router.handle(from([1, 1, 1, 1]));
        assert_eq!(
            (limited.status, limited.header("Retry-After")),
            (429, Some("1"))
        );
        assert_eq!(router.handle(from([2, 2, 2, 2])).status, 200); // its own bucket

        *now.lock().unwrap() += Duration::from_secs(1);
        assert_eq!(router.handle(from([1, 1, 1, 1])).status, 200);
    }
}
