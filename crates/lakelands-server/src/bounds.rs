//! The HTTP door's bounds: how many requests a connection and a principal
//! may send in a second, how many of a principal's are answered at once,
//! and how large a body is read whole
//! (`docs/decisions/095-the-http-door-bounds-what-one-caller-spends.md`).
//!
//! The door counts a request where it learns who sent it: by its
//! connection as it arrives, and by its principal where the door resolves
//! one ([`crate::http::resolve`], [`crate::http::forwarded`], an MCP
//! session's lookup). A request refused before that is counted by its
//! connection only. A principal's slot is a [`Held`], given back by its
//! drop: when the request is answered, cancelled or panics.
//!
//! No lock here is held across an `.await`: the counts sit behind a plain
//! mutex, taken to count and let go at once.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use lakelands_api::Principal;
use lakelands_api::error::{Per, Situation};
use lakelands_api::id::LakeId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A bound, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// Nothing is bounded: `"off"` in the configuration.
    Off,
    /// At most this many.
    At(NonZeroU64),
}

impl Limit {
    const fn at(n: u64) -> Limit {
        match NonZeroU64::new(n) {
            Some(n) => Limit::At(n),
            None => Limit::Off,
        }
    }

    fn get(self) -> Option<u64> {
        match self {
            Limit::Off => None,
            Limit::At(n) => Some(n.get()),
        }
    }
}

impl Serialize for Limit {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Limit::Off => s.serialize_str("off"),
            Limit::At(n) => s.serialize_u64(n.get()),
        }
    }
}

impl<'de> Deserialize<'de> for Limit {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Limit, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Said {
            At(NonZeroU64),
            Word(String),
        }
        match Said::deserialize(d) {
            Ok(Said::At(n)) => Ok(Limit::At(n)),
            Ok(Said::Word(w)) if w == "off" => Ok(Limit::Off),
            _ => Err(serde::de::Error::custom(
                "a bound is a count above zero, or \"off\"",
            )),
        }
    }
}

/// The `bounds` of the `server` section: each a [`Limit`], each `"off"`
/// for none. A section that names none of them has the defaults, which
/// no caller that waits for its answers meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bounds {
    /// The requests a connection may send in a second.
    pub connection_rate: Limit,
    /// The requests a principal may send in a second, over all its
    /// connections.
    pub principal_rate: Limit,
    /// The requests of a principal answered at once.
    pub principal_in_flight: Limit,
    /// The largest body read whole, in bytes.
    pub body_bytes: Limit,
}

impl Bounds {
    /// Nothing bounded.
    pub const OFF: Bounds = Bounds {
        connection_rate: Limit::Off,
        principal_rate: Limit::Off,
        principal_in_flight: Limit::Off,
        body_bytes: Limit::Off,
    };
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds {
            connection_rate: Limit::at(1000),
            principal_rate: Limit::at(1000),
            principal_in_flight: Limit::at(128),
            body_bytes: Limit::at(4 << 20),
        }
    }
}

/// A count of requests a second: a bucket that holds one second's worth,
/// `most`, full at the start and filling at `most` a second.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rate {
    tokens: f64,
    at: Instant,
}

impl Rate {
    pub(crate) fn new(now: Instant) -> Rate {
        Rate {
            tokens: f64::MAX,
            at: now,
        }
    }

    fn fill(&mut self, most: u64, now: Instant) {
        let most = most as f64;
        let since = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens.min(most) + since * most).min(most);
        self.at = now;
    }

    /// Take one request, or answer the whole seconds until one may come.
    pub(crate) fn take(&mut self, most: u64, now: Instant) -> Result<(), u64> {
        self.fill(most, now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }
        let wait = (1.0 - self.tokens) / most as f64;
        Err((wait.ceil() as u64).max(1))
    }

    /// Whether it is full again: a count no different from a new one.
    fn full(&self, most: u64, now: Instant) -> bool {
        let mut r = *self;
        r.fill(most, now);
        r.tokens >= most as f64
    }
}

/// Why a request was refused by a bound, and the seconds to wait.
pub(crate) struct Over {
    pub(crate) situation: Box<Situation>,
    pub(crate) retry_after: u64,
}

fn over_rate(per: Per, most: u64, retry_after: u64) -> Over {
    Over {
        situation: Box::new(Situation::RateBound {
            per,
            most,
            retry_after,
        }),
        retry_after,
    }
}

/// A connection's own count: one per connection, dropped with it.
pub(crate) struct Connection {
    rate: Mutex<Rate>,
}

impl Connection {
    pub(crate) fn new() -> Connection {
        Connection {
            rate: Mutex::new(Rate::new(Instant::now())),
        }
    }
}

/// One principal's counts.
struct Count {
    rate: Rate,
    in_flight: u64,
}

/// The counts the door keeps, and the bounds it holds them to.
pub(crate) struct Gate {
    pub(crate) bounds: Bounds,
    principals: Mutex<Principals>,
}

#[derive(Default)]
struct Principals {
    by: HashMap<(LakeId, String), Count>,
    /// How many counts there may be before the idle ones are dropped.
    sweep_at: usize,
}

/// The fewest counts kept before any is dropped.
const SWEEP_LEAST: usize = 64;

impl Gate {
    pub(crate) fn new(bounds: Bounds) -> Arc<Gate> {
        Arc::new(Gate {
            bounds,
            principals: Mutex::new(Principals {
                by: HashMap::new(),
                sweep_at: SWEEP_LEAST,
            }),
        })
    }

    /// Count a request of `connection`'s.
    pub(crate) fn connection(&self, connection: &Connection) -> Result<(), Over> {
        let Some(most) = self.bounds.connection_rate.get() else {
            return Ok(());
        };
        let mut rate = connection.rate.lock().unwrap_or_else(|e| e.into_inner());
        (rate.take(most, Instant::now())).map_err(|wait| over_rate(Per::Connection, most, wait))
    }

    /// Count a request of `who`'s: a slot held until the request ends.
    ///
    /// A count that is idle, none of its requests in flight and its rate
    /// full again, says nothing a new one would not, and is dropped: when
    /// its last request ends, and, for those never full again by then,
    /// whenever the counts have doubled since they were last swept. So
    /// the map holds the principals active within the last second or so,
    /// at most twice over.
    pub(crate) fn principal(self: &Arc<Self>, who: &Principal) -> Result<Held, Over> {
        let (rate, in_flight) = (self.bounds.principal_rate, self.bounds.principal_in_flight);
        if rate == Limit::Off && in_flight == Limit::Off {
            return Ok(Held {
                gate: self.clone(),
                key: None,
            });
        }
        let now = Instant::now();
        let key = (who.lake.clone(), who.id.clone());
        let mut p = self.principals.lock().unwrap_or_else(|e| e.into_inner());
        if !p.by.contains_key(&key) && p.by.len() >= p.sweep_at {
            p.by.retain(|_, c| c.in_flight > 0 || !rate.get().is_none_or(|m| c.rate.full(m, now)));
            p.sweep_at = (p.by.len() * 2).max(SWEEP_LEAST);
        }
        let count = (p.by.entry(key.clone())).or_insert_with(|| Count {
            rate: Rate::new(now),
            in_flight: 0,
        });
        if let Some(most) = in_flight.get()
            && count.in_flight >= most
        {
            return Err(Over {
                situation: Box::new(Situation::InFlightBound { most }),
                retry_after: 1,
            });
        }
        if let Some(most) = rate.get() {
            (count.rate.take(most, now)).map_err(|wait| over_rate(Per::Principal, most, wait))?;
        }
        count.in_flight += 1;
        Ok(Held {
            gate: self.clone(),
            key: Some(key),
        })
    }
}

/// A principal's slot for one request, given back when it is dropped.
/// It owns its gate, so a request whose work outlives its handler, an
/// ingest's commits, takes it along.
pub(crate) struct Held {
    gate: Arc<Gate>,
    key: Option<(LakeId, String)>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        let rate = self.gate.bounds.principal_rate.get();
        let mut p = (self.gate.principals.lock()).unwrap_or_else(|e| e.into_inner());
        if let Some(count) = p.by.get_mut(&key) {
            count.in_flight = count.in_flight.saturating_sub(1);
            if count.in_flight == 0 && rate.is_none_or(|m| count.rate.full(m, Instant::now())) {
                p.by.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests;
