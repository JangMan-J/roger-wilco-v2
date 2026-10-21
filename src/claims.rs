//! Claims: exclusive ownership of one key by one session, renewed by that
//! session alone, freed by release or by TTL expiry.
//!
//! Expiry is never found by a scan. Each grant records an absolute deadline,
//! and every read and every decision compares that deadline with the time it
//! is given. A held entry past its deadline is simply not live; the next
//! acquire overwrites it. Keys are opaque strings, stored and returned
//! verbatim.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub owner: String,
    /// Absolute deadline in coordinator milliseconds since the Unix epoch.
    pub expires_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Acquire,
    Renew,
    Release,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        match s {
            "acquire" => Some(Op::Acquire),
            "renew" => Some(Op::Renew),
            "release" => Some(Op::Release),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Op::Acquire => "acquire",
            Op::Renew => "renew",
            Op::Release => "release",
        }
    }
}

/// What a decision does to stored state. The caller makes it durable, then
/// applies it, then answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Grant { owner: String, expires_ms: i64 },
    Release,
}

/// The answer to one request: the change to commit (if any), and the live
/// holder of the key once that change is applied (`None` when free).
#[derive(Debug, PartialEq)]
pub struct Decision {
    pub ok: bool,
    pub change: Option<Change>,
    pub holder: Option<Held>,
}

#[derive(Default)]
pub struct Claims {
    held: BTreeMap<String, Held>,
}

impl Claims {
    /// The holder of `key` at `now`, if its deadline has not passed.
    pub fn live(&self, key: &str, now: i64) -> Option<&Held> {
        self.held.get(key).filter(|h| now < h.expires_ms)
    }

    /// Decide `op` on `key` for `owner` at `now`. Acquire succeeds when the key
    /// is free, expired, or already the owner's (a retry refreshes it). Renew
    /// and release succeed only for the live holder: a session that lost its
    /// claim to expiry cannot renew it back.
    pub fn decide(&self, op: &Op, key: &str, owner: &str, ttl_ms: i64, now: i64) -> Decision {
        let live = self.live(key, now).cloned();
        let mine = live.as_ref().is_some_and(|h| h.owner == owner);
        let allowed = match op {
            Op::Acquire => live.is_none() || mine,
            Op::Renew | Op::Release => mine,
        };
        if !allowed {
            return Decision {
                ok: false,
                change: None,
                holder: live,
            };
        }
        match op {
            Op::Release => Decision {
                ok: true,
                change: Some(Change::Release),
                holder: None,
            },
            Op::Acquire | Op::Renew => {
                let held = Held {
                    owner: owner.to_string(),
                    expires_ms: now.saturating_add(ttl_ms),
                };
                Decision {
                    ok: true,
                    change: Some(Change::Grant {
                        owner: held.owner.clone(),
                        expires_ms: held.expires_ms,
                    }),
                    holder: Some(held),
                }
            }
        }
    }

    /// Apply a committed change, whether just decided or replayed from the log.
    pub fn apply(&mut self, key: &str, change: &Change) {
        match change {
            Change::Grant { owner, expires_ms } => {
                self.held.insert(
                    key.to_string(),
                    Held {
                        owner: owner.clone(),
                        expires_ms: *expires_ms,
                    },
                );
            }
            Change::Release => {
                self.held.remove(key);
            }
        }
    }

    /// Every live claim at `now`, in key order.
    pub fn active(&self, now: i64) -> Vec<(&str, &Held)> {
        self.held
            .iter()
            .filter(|(_, h)| now < h.expires_ms)
            .map(|(k, h)| (k.as_str(), h))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "[[153:Meteor Shower:7]]";

    fn commit(c: &mut Claims, op: Op, owner: &str, ttl: i64, now: i64) -> Decision {
        let d = c.decide(&op, KEY, owner, ttl, now);
        if let Some(ch) = &d.change {
            c.apply(KEY, ch);
        }
        d
    }

    #[test]
    fn second_owner_is_refused_and_told_the_holder() {
        let mut c = Claims::default();
        assert!(commit(&mut c, Op::Acquire, "a", 1000, 0).ok);
        let d = commit(&mut c, Op::Acquire, "b", 1000, 10);
        assert!(!d.ok);
        assert_eq!(d.change, None);
        assert_eq!(d.holder.unwrap().owner, "a");
    }

    #[test]
    fn only_the_holder_renews_and_renewal_moves_the_deadline() {
        let mut c = Claims::default();
        commit(&mut c, Op::Acquire, "a", 1000, 0);
        assert!(!commit(&mut c, Op::Renew, "b", 1000, 500).ok);
        assert!(commit(&mut c, Op::Renew, "a", 1000, 500).ok);
        assert_eq!(c.live(KEY, 1200).unwrap().owner, "a");
        assert!(c.live(KEY, 1500).is_none());
    }

    #[test]
    fn expiry_frees_the_key_without_any_sweep() {
        let mut c = Claims::default();
        commit(&mut c, Op::Acquire, "a", 1000, 0);
        assert!(c.live(KEY, 999).is_some());
        assert!(c.live(KEY, 1000).is_none());
        assert!(c.active(1000).is_empty());
        assert!(!commit(&mut c, Op::Renew, "a", 1000, 1000).ok);
        assert!(commit(&mut c, Op::Acquire, "b", 1000, 1000).ok);
        assert_eq!(c.active(1001), vec![(KEY, c.live(KEY, 1001).unwrap())]);
        assert_eq!(c.live(KEY, 1001).unwrap().owner, "b");
    }

    #[test]
    fn release_is_the_holders_alone() {
        let mut c = Claims::default();
        commit(&mut c, Op::Acquire, "a", 1000, 0);
        assert!(!commit(&mut c, Op::Release, "b", 0, 1).ok);
        assert!(commit(&mut c, Op::Release, "a", 0, 1).ok);
        assert!(c.live(KEY, 2).is_none());
        assert!(commit(&mut c, Op::Acquire, "b", 1000, 2).ok);
    }
}
