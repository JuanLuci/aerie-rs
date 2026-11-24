//! The player list and name plates (spec 27): what each player's ping is
//! and how it is measured, the order and timing of the list packets, and the
//! name plate a player shows. Pure logic: `world.rs` holds the players and
//! calls in.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use aerie_protocol::proto;
use uuid::Uuid;

/// Packet ids (gen/packets.tsv).
pub mod id {
    pub const PING: u32 = 3;
    pub const ADD: u32 = 224;
    pub const REMOVE: u32 = 225;
    pub const PING_UPDATE: u32 = 227;
}

/// Seconds between the pings a client is sent (counted in world ticks).
const PING_EVERY: f32 = 1.0;
/// How often every player is sent every player's ping.
pub const UPDATE_EVERY: Duration = Duration::from_secs(10);
/// A ping average looks at the pongs of this long.
const WINDOW: Duration = Duration::from_secs(1);

/// The three ways the client answers a ping (`Pong.type`).
pub const KINDS: usize = 3;

/// What one player's connection knows about its latency.
#[derive(Default)]
pub struct Latency {
    /// Pings sent and not answered yet, per kind: (id, when).
    sent: [VecDeque<(i32, Instant)>; KINDS],
    /// Answers of the last second per kind: (when, microseconds).
    window: [VecDeque<(Instant, i64)>; KINDS],
    /// The average of that window as of the last answer, per kind.
    average: [f64; KINDS],
    /// The last answer's microseconds, per kind.
    last: [i64; KINDS],
    /// Seconds until the next ping.
    timer: f32,
    /// Where the random ping ids come from.
    seed: u64,
}

impl Latency {
    /// Counts `dt` seconds; when a ping is due, the packet to send. The first
    /// one is due at once and the next ones a second of ticks later.
    pub fn tick(&mut self, dt: f32, now: Instant) -> Option<(u32, Vec<u8>)> {
        self.timer -= dt;
        if self.timer > 0.0 {
            return None;
        }
        self.timer = PING_EVERY;
        let id = self.next_id();
        for q in &mut self.sent {
            q.push_back((id, now));
        }
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let ping = proto::Ping {
            id,
            time: proto::InstantData {
                seconds: time.as_secs() as i64,
                nanos: time.subsec_nanos() as i32,
            },
            last_ping_value_raw: self.last[0] as i32,
            last_ping_value_direct: self.last[1] as i32,
            last_ping_value_tick: self.last[2] as i32,
        };
        let mut out = Vec::new();
        aerie_protocol::wire::Wire::encode(&ping, &mut out).ok()?;
        Some((id::PING, out))
    }

    /// The client answered ping `id` of this kind at `now`.
    pub fn pong(&mut self, kind: usize, id: i32, now: Instant) {
        let Some(k) = (kind < KINDS).then_some(kind) else {
            return;
        };
        // The official server takes the oldest unanswered ping and refuses
        // an answer for another one; Aerie ignores it.
        let Some((sent_id, sent)) = self.sent[k].pop_front() else {
            return;
        };
        let elapsed = now.saturating_duration_since(sent).as_nanos() as i64;
        if sent_id != id || elapsed <= 0 {
            return;
        }
        let micros = elapsed / 1000;
        let w = &mut self.window[k];
        while w
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > WINDOW)
        {
            w.pop_front();
        }
        w.push_back((now, micros));
        self.average[k] = w.iter().map(|(_, v)| *v as f64).sum::<f64>() / w.len() as f64;
        self.last[k] = micros;
    }

    /// The number the list shows: the one-second average of the direct
    /// answers, in whole milliseconds (0 until one arrived).
    pub fn list_ping(&self) -> i32 {
        (self.average[1].ceil() as i64 / 1000) as i32
    }

    fn next_id(&mut self) -> i32 {
        if self.seed == 0 {
            self.seed = rand::random::<u64>() | 1;
        }
        // xorshift: the ids only have to differ.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 16) as i32
    }
}

/// A player as the list shows it.
#[derive(Clone)]
pub struct Info {
    pub uuid: Uuid,
    pub name: String,
    pub ping: i32,
    /// Whether it is in the world (the list then names the world).
    pub in_world: bool,
}

impl Info {
    fn entry(&self, world: Uuid) -> proto::ServerPlayerListPlayer {
        proto::ServerPlayerListPlayer {
            uuid: self.uuid,
            username: Some(self.name.clone()),
            world_uuid: self.in_world.then_some(world),
            ping: self.ping,
            spectating: false,
            lives_remaining: None,
        }
    }
}

/// `Add` for these players.
pub fn add(players: &[Info], world: Uuid) -> Option<(u32, Vec<u8>)> {
    let p = proto::AddToServerPlayerList {
        players: players.iter().map(|i| i.entry(world)).collect(),
    };
    let mut out = Vec::new();
    aerie_protocol::wire::Wire::encode(&p, &mut out).ok()?;
    Some((id::ADD, out))
}

/// `Remove` for one player.
pub fn remove(uuid: Uuid) -> Option<(u32, Vec<u8>)> {
    let p = proto::RemoveFromServerPlayerList {
        players: vec![uuid],
    };
    let mut out = Vec::new();
    aerie_protocol::wire::Wire::encode(&p, &mut out).ok()?;
    Some((id::REMOVE, out))
}

/// Server-wide state of the list.
pub struct State {
    /// When every player is next sent every ping.
    next_update: Instant,
    /// The most players there were at once (sets the order, below).
    peak: usize,
}

impl Default for State {
    fn default() -> Self {
        State {
            next_update: Instant::now() + UPDATE_EVERY,
            peak: 0,
        }
    }
}

impl State {
    /// Records how many players there are now.
    pub fn count(&mut self, n: usize) {
        self.peak = self.peak.max(n);
    }

    /// Whether the ping update is due; the next one is `UPDATE_EVERY` after.
    pub fn update_due(&mut self, now: Instant) -> bool {
        if now < self.next_update {
            return false;
        }
        self.next_update = now + UPDATE_EVERY;
        true
    }

    /// `players` (in join order) in the order the official server lists them:
    /// by the bucket of a hash table that has grown with the most players
    /// there ever were.
    pub fn listing_order(&self, players: &[Info]) -> Vec<Info> {
        let mut cap = 16usize;
        while self.peak.max(players.len()) * 4 >= cap * 3 {
            cap *= 2;
        }
        let mut out = players.to_vec();
        out.sort_by_key(|i| {
            let h = java_uuid_hash(i.uuid) as u32;
            ((h ^ (h >> 16)) & 0x7fff_ffff) as usize & (cap - 1)
        });
        out
    }
}

/// `UpdateServerPlayerListPing` for these players (in listing order): the
/// pairs go in the order the official server's open-addressing map walks
/// them.
pub fn ping_update(players: &[Info]) -> Option<(u32, Vec<u8>)> {
    let keys: Vec<Uuid> = players.iter().map(|i| i.uuid).collect();
    let pings = map_order(&keys)
        .into_iter()
        .filter_map(|u| players.iter().find(|i| i.uuid == u).map(|i| (u, i.ping)))
        .collect();
    let p = proto::UpdateServerPlayerListPing { players: pings };
    let mut out = Vec::new();
    aerie_protocol::wire::Wire::encode(&p, &mut out).ok()?;
    Some((id::PING_UPDATE, out))
}

fn java_uuid_hash(u: Uuid) -> i32 {
    let v = u.as_u128();
    let hilo = (v >> 64) as u64 ^ v as u64;
    (hilo >> 32) as i32 ^ hilo as i32
}

/// The iteration order of a Java `Object2IntOpenHashMap<UUID>` built with
/// the expected size `keys.len()` and these keys put in this order: linear
/// probing from a mixed hash, growing at three quarters full, walked from
/// the last slot down.
pub fn map_order(keys: &[Uuid]) -> Vec<Uuid> {
    const LOAD: f64 = 0.75;
    let table_size = |expected: usize| {
        ((expected as f64 / LOAD).ceil() as usize)
            .next_power_of_two()
            .max(2)
    };
    let max_fill = |n: usize| (((n as f64) * LOAD).ceil() as usize).min(n - 1);
    let mix = |u: Uuid| {
        let h = java_uuid_hash(u).wrapping_mul(0x9E37_79B9_u32 as i32) as u32;
        (h ^ (h >> 16)) as usize
    };
    let place = |table: &mut Vec<Option<Uuid>>, u: Uuid| {
        let mask = table.len() - 1;
        let mut pos = mix(u) & mask;
        while table[pos].is_some() {
            pos = (pos + 1) & mask;
        }
        table[pos] = Some(u);
    };
    let mut n = table_size(keys.len());
    let mut table: Vec<Option<Uuid>> = vec![None; n];
    let mut size = 0usize;
    for &u in keys {
        place(&mut table, u);
        let before = size;
        size += 1;
        if before >= max_fill(n) {
            n = table_size(size + 1);
            let mut bigger = vec![None; n];
            for old in table.iter().rev().flatten() {
                place(&mut bigger, *old);
            }
            table = bigger;
        }
    }
    table.into_iter().rev().flatten().collect()
}

/// The name plate of a player: its text (absent when removed) and what
/// viewers have yet to be told.
pub struct Plate {
    text: Option<String>,
    outdated: bool,
    removed: bool,
}

/// A change viewers are told.
#[derive(Debug, PartialEq)]
pub enum Change {
    Text(String),
    Removed,
}

impl Plate {
    pub fn new(text: &str) -> Self {
        Plate {
            text: Some(text.to_owned()),
            outdated: true,
            removed: false,
        }
    }

    /// Sets the text; nothing changes (nothing is sent) when it is the same.
    pub fn set(&mut self, text: &str) {
        match &mut self.text {
            Some(t) if t == text => {}
            Some(t) => {
                *t = text.to_owned();
                self.outdated = true;
            }
            None => {
                // A new plate is always sent, even an empty one.
                self.text = Some(text.to_owned());
                self.outdated = true;
            }
        }
    }

    pub fn remove(&mut self) {
        if self.text.take().is_some() {
            self.removed = true;
            self.outdated = false;
        }
    }

    /// What to tell viewers now, once.
    pub fn take(&mut self) -> Option<Change> {
        if self.outdated {
            self.outdated = false;
            self.removed = false;
            return self.text.clone().map(Change::Text);
        }
        if std::mem::take(&mut self.removed) {
            return Some(Change::Removed);
        }
        None
    }
}

/// The name plate update of a text.
pub fn plate_update(text: &str) -> proto::ComponentUpdate {
    proto::ComponentUpdate::NameplateUpdate(Box::new(proto::NameplateUpdate {
        text: text.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn map_order_matches_the_official_two_player_list() {
        // Two players that collide: the later one sits after the earlier
        // one and the walk goes down, so it comes first.
        assert_eq!(map_order(&[u(0xa2), u(0xa3)]), vec![u(0xa3), u(0xa2)]);
        assert_eq!(map_order(&[u(0xa3), u(0xa2)]), vec![u(0xa2), u(0xa3)]);
        assert_eq!(map_order(&[]), Vec::<Uuid>::new());
    }

    #[test]
    fn map_order_keeps_every_key_when_it_grows() {
        let keys: Vec<Uuid> = (1..40u128).map(|i| u(i * 7919)).collect();
        let mut got = map_order(&keys);
        got.sort();
        let mut want = keys.clone();
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn ping_is_the_one_second_average_in_whole_milliseconds() {
        let mut l = Latency::default();
        let t0 = Instant::now();
        // Without pings sent an answer counts for nothing.
        l.pong(1, 5, t0);
        assert_eq!(l.list_ping(), 0);
        let p = l.tick(0.0, t0).unwrap();
        assert_eq!(p.1.len(), 28);
        let (ping, _) = <proto::Ping as aerie_protocol::wire::Wire>::decode(&p.1, 0).unwrap();
        for k in 0..3 {
            l.pong(k, ping.id, t0 + Duration::from_micros(12_500));
        }
        assert_eq!(l.list_ping(), 12);
        assert_eq!(l.last, [12_500; 3]);
        // The next ping is a second of ticks away.
        assert!(l.tick(0.5, t0).is_none());
        assert!(l.tick(0.5, t0).is_some());
    }

    #[test]
    fn a_wrong_answer_is_ignored() {
        let mut l = Latency::default();
        let t0 = Instant::now();
        l.tick(0.0, t0).unwrap();
        l.pong(1, 12345, t0 + Duration::from_millis(5));
        assert_eq!(l.list_ping(), 0);
    }

    #[test]
    fn plate_tells_only_changes() {
        let mut p = Plate::new("A");
        assert_eq!(p.take(), Some(Change::Text("A".into())));
        assert_eq!(p.take(), None);
        p.set("A");
        assert_eq!(p.take(), None);
        p.set("B");
        assert_eq!(p.take(), Some(Change::Text("B".into())));
        p.remove();
        assert_eq!(p.take(), Some(Change::Removed));
        assert_eq!(p.take(), None);
        p.set("");
        assert_eq!(p.take(), Some(Change::Text(String::new())));
    }

    #[test]
    fn listing_follows_the_bucket_order() {
        let s = State::default();
        let info = |n: u128| Info {
            uuid: u(n),
            name: String::new(),
            ping: 0,
            in_world: true,
        };
        let order: Vec<Uuid> = s
            .listing_order(&[info(3), info(1), info(2)])
            .into_iter()
            .map(|i| i.uuid)
            .collect();
        assert_eq!(order, vec![u(1), u(2), u(3)]);
    }
}
