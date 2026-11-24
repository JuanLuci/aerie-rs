//! The teleport commands (spec 30): `/tp` in all its forms, `/spawn` and
//! `/warp`, the per-player teleport history and the saved warps.
//!
//! Words, types and counts were checked by `commands`; what reaches `run`
//! is a valid line. The world is reached through `Host`, so the logic can
//! be tested without one.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::messages::Msg;

/// Where the world's players start (and `/spawn`, `/tp home` and
/// `/tp world` go): the built world's spawn, facing 3 pi / 2.
pub const SPAWN: [f64; 3] = [16.0, 66.0, 16.0];
/// Pitch, yaw, roll of the spawn point.
pub const SPAWN_HEAD: [f32; 3] = [0.0, 4.712_389, 0.0];
/// The one world's name.
const WORLD: &str = "default";
/// Teleports a history keeps.
const HISTORY_MAX: usize = 100;
/// Warps on one page of `/warp list`.
const WARPS_PER_PAGE: usize = 8;
/// The words of `/warp`: they cannot name a warp.
const RESERVED: [&str; 5] = ["go", "list", "remove", "reload", "set"];
/// What the official server's float arithmetic uses: degrees to radians and
/// back, as 32-bit floats.
const TO_RAD: f32 = (std::f64::consts::PI / 180.0) as f32;
const TO_DEG: f32 = 180.0 / (std::f64::consts::PI as f32);

// ---------------------------------------------------------------------
// Words

/// A coordinate word: a number, with marks in front: `~` relative to where
/// the player is, `_` the ground height (plus the number), `c` the number
/// is in chunks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coord {
    value: f64,
    height: bool,
    relative: bool,
    chunk: bool,
}

impl Coord {
    /// Reads a word as the official server does; `None` when the number
    /// after the marks is not one.
    pub fn parse(s: &str) -> Option<Coord> {
        let (mut height, mut relative, mut chunk) = (false, false, false);
        let mut rest = s;
        loop {
            match rest.chars().next() {
                Some('_') => {
                    rest = &rest[1..];
                    height = true;
                }
                Some('~') => {
                    rest = &rest[1..];
                    relative = true;
                }
                Some('c') => {
                    rest = &rest[1..];
                    chunk = true;
                    // Nothing is looked at after a chunk mark but the number.
                    if rest.is_empty() {
                        return Some(Coord {
                            value: 0.0,
                            height,
                            relative,
                            chunk,
                        });
                    }
                    break;
                }
                None => {
                    return Some(Coord {
                        value: 0.0,
                        height,
                        relative,
                        chunk,
                    });
                }
                Some(_) => break,
            }
            if rest.is_empty() {
                return Some(Coord {
                    value: 0.0,
                    height,
                    relative,
                    chunk,
                });
            }
        }
        Some(Coord {
            value: java_double(rest)?,
            height,
            relative,
            chunk,
        })
    }

    fn resolve(&self, base: f64) -> f64 {
        let v = if self.chunk {
            self.value * 32.0
        } else {
            self.value
        };
        if self.relative { v + base } else { v }
    }

    /// The x or z it names, `base` being where the player is.
    pub fn xz(&self, base: f64) -> f64 {
        self.resolve(base)
    }
}

/// A rotation word in degrees: a number, with `~` anywhere meaning
/// relative to the current one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelFloat {
    value: f32,
    relative: bool,
}

impl RelFloat {
    /// Reads a word; the error is the text the official server prints
    /// (the word without its `~`).
    pub fn parse(s: &str) -> Result<RelFloat, String> {
        let relative = s.contains('~');
        let t = s.replace('~', "");
        let value = if t.trim().is_empty() {
            0.0
        } else {
            java_double(&t).ok_or_else(|| t.clone())? as f32
        };
        Ok(RelFloat { value, relative })
    }

    fn resolve(&self, base: f32) -> f32 {
        if self.relative {
            base + self.value
        } else {
            self.value
        }
    }
}

/// Java's `Double.parseDouble` for the words a player can type: optional
/// spaces, a sign, digits, a point, an exponent, a trailing `f` or `d`;
/// `NaN` and `Infinity`.
pub fn java_double(s: &str) -> Option<f64> {
    let t = s.trim();
    match t {
        "NaN" | "+NaN" | "-NaN" => return Some(f64::NAN),
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let u = t.trim_end_matches(['f', 'F', 'd', 'D']);
    if u.is_empty() || u.contains(['n', 'N', 'i', 'I']) || t.len() - u.len() > 1 {
        return None;
    }
    u.parse::<f64>().ok()
}

/// Java's `Double.toString`: the shortest digits that read back the same,
/// plain between 0.001 and 10 million, `1.0E10` outside.
pub fn java_string(d: f64) -> String {
    if d.is_nan() {
        return "NaN".into();
    }
    if d.is_infinite() {
        return if d > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if d == 0.0 {
        return if d.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let a = d.abs();
    if (1e-3..1e7).contains(&a) {
        let s = format!("{d}");
        return if s.contains('.') { s } else { format!("{s}.0") };
    }
    let e = format!("{d:e}");
    let (m, x) = e.split_once('e').unwrap_or((&e, "0"));
    let m = if m.contains('.') {
        m.to_owned()
    } else {
        format!("{m}.0")
    };
    format!("{m}E{x}")
}

// ---------------------------------------------------------------------
// Where a player is, and a move

/// A player's place: position, body and head rotation (pitch, yaw, roll).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: [f64; 3],
    pub body: [f32; 3],
    pub head: [f32; 3],
}

/// A teleport: a field that is NaN stays as it was.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    pub pos: [f64; 3],
    pub body: [f32; 3],
    pub head: [f32; 3],
}

const NAN3: [f32; 3] = [f32::NAN; 3];

impl Move {
    /// To a position facing `head`, the body turned to its yaw and level.
    pub fn facing(pos: [f64; 3], head: [f32; 3]) -> Move {
        Move {
            pos,
            body: [0.0, head[1], 0.0],
            head,
        }
    }

    /// The pose it makes of `from`.
    pub fn apply(&self, from: &Pose) -> Pose {
        let keep = |new: [f32; 3], old: [f32; 3]| {
            [0, 1, 2].map(|i| if new[i].is_finite() { new[i] } else { old[i] })
        };
        Pose {
            pos: [0, 1, 2].map(|i| {
                if self.pos[i].is_nan() {
                    from.pos[i]
                } else {
                    self.pos[i]
                }
            }),
            // The body keeps what is NaN; so does the head, and it also
            // keeps what is infinite.
            body: [0, 1, 2].map(|i| {
                if self.body[i].is_nan() {
                    from.body[i]
                } else {
                    self.body[i]
                }
            }),
            head: keep(self.head, from.head),
        }
    }
}

/// The rotation words of `/tp x y z`: degrees as given, each possibly
/// relative; what was not given stays.
struct Look {
    yaw: Option<RelFloat>,
    pitch: Option<RelFloat>,
    roll: Option<RelFloat>,
}

impl Look {
    fn from(a: &Args) -> Look {
        let get = |n: &str| a.opt(n).and_then(|v| RelFloat::parse(v).ok());
        Look {
            yaw: get("yaw"),
            pitch: get("pitch"),
            roll: get("roll"),
        }
    }

    fn any(&self) -> bool {
        self.yaw.is_some() || self.pitch.is_some() || self.roll.is_some()
    }

    /// Radians, NaN when not given; relative ones count from the head.
    fn radians(&self, head: [f32; 3]) -> (f32, f32, f32) {
        let one = |r: &Option<RelFloat>, base: f32| match r {
            Some(r) => r.resolve(base * TO_DEG) * TO_RAD,
            None => f32::NAN,
        };
        (
            one(&self.yaw, head[1]),
            one(&self.pitch, head[0]),
            one(&self.roll, head[2]),
        )
    }
}

/// Degrees of a rotation in radians, or of the head when it was NaN.
fn degrees(r: f32, head: f32) -> f64 {
    f64::from(if r.is_nan() { head * TO_DEG } else { r * TO_DEG })
}

// ---------------------------------------------------------------------
// Teleports the client has not confirmed yet

/// The teleports sent to a client and not yet acknowledged. Until all are,
/// the position and rotation it sends are about the place it was moved
/// from, and the server ignores them.
#[derive(Debug, Default)]
pub struct Acks(VecDeque<(i8, [f64; 3])>);

impl Acks {
    /// A teleport was sent (its id and where it puts the player).
    pub fn queue(&mut self, id: i8, pos: [f64; 3]) {
        self.0.push_back((id, pos));
    }

    /// The client acknowledged `id` standing at `pos`: true when that is
    /// the oldest teleport waiting and the place is the one sent.
    pub fn ack(&mut self, id: i8, pos: [f64; 3]) -> bool {
        let Some(&(want, at)) = self.0.front() else {
            return false;
        };
        if want != id {
            return false;
        }
        // The client holds positions as 32-bit floats: about half an ulp of
        // each coordinate more than the fixed tolerance.
        let ulp = |v: f64| {
            let e = if v == 0.0 { -1022 } else { v.abs().log2().floor() as i32 };
            2f64.powi(e - 24)
        };
        let precision = (0..3).map(|i| ulp(at[i]).powi(2)).sum::<f64>().sqrt();
        let max = 0.001f64.sqrt() + precision;
        let d: f64 = (0..3).map(|i| (at[i] - pos[i]).powi(2)).sum();
        if d > max * max {
            return false;
        }
        self.0.pop_front();
        true
    }

    /// Whether the client still has to confirm one.
    pub fn waiting(&self) -> bool {
        !self.0.is_empty()
    }
}

// ---------------------------------------------------------------------
// History

/// A place a player was before a teleport.
#[derive(Clone, Debug, PartialEq)]
pub struct Waypoint {
    pub pos: [f64; 3],
    pub head: [f32; 3],
    pub message: String,
}

/// The places before the player's teleports (`back`, newest first) and the
/// ones it came back from (`forward`).
#[derive(Clone, Debug, Default)]
pub struct History {
    back: VecDeque<Waypoint>,
    forward: VecDeque<Waypoint>,
}

/// What `back` or `forward` did.
#[derive(Debug, PartialEq)]
pub enum Went {
    /// Nothing to go to.
    NotFurther,
    /// To this place; entries left that way and the other way.
    To(Waypoint, usize, usize),
}

impl History {
    /// Remembers a place before a teleport; what could be gone forward to
    /// is forgotten.
    pub fn append(&mut self, pos: [f64; 3], head: [f32; 3], message: &str) {
        self.back.push_front(Waypoint {
            pos,
            head,
            message: message.to_owned(),
        });
        self.forward.clear();
        while self.back.len() > HISTORY_MAX {
            self.back.pop_back();
        }
    }

    pub fn sizes(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    /// Takes up to `count` entries off one list (the last one is where the
    /// player goes) and puts where it stands on the other.
    pub fn go(&mut self, forward: bool, count: usize, here: ([f64; 3], [f32; 3])) -> Went {
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        let mut point = None;
        for _ in 0..count {
            match from.pop_front() {
                Some(p) => point = Some(p),
                None => break,
            }
        }
        let Some(point) = point else {
            return Went::NotFurther;
        };
        to.push_front(Waypoint {
            pos: here.0,
            head: here.1,
            message: point.message.clone(),
        });
        let (left, other) = (from.len(), to.len());
        Went::To(point, left, other)
    }
}

// ---------------------------------------------------------------------
// Warps

#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    /// The name as it was typed when set.
    pub id: String,
    pub creator: String,
    /// Milliseconds since 1970.
    pub created: i64,
    pub pos: [f64; 3],
    pub head: [f32; 3],
}

/// The warps, by lower-cased name, and the file they are kept in.
#[derive(Default)]
pub struct Warps {
    map: HashMap<String, Warp>,
    file: Option<PathBuf>,
}

impl Warps {
    /// Reads `file` (`created x y z pitch yaw roll creator id`, tab
    /// separated, one warp a line) and keeps it up to date from then on.
    pub fn open(file: PathBuf) -> Warps {
        let mut w = Warps {
            map: HashMap::new(),
            file: Some(file),
        };
        w.reload();
        w
    }

    /// Reads the file again; the number of warps.
    pub fn reload(&mut self) -> usize {
        self.map.clear();
        if let Some(text) = self
            .file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
        {
            for l in text.lines() {
                if let Some(w) = Warp::parse(l) {
                    self.map.insert(w.id.to_lowercase(), w);
                }
            }
        }
        self.map.len()
    }

    fn save(&self) {
        let Some(f) = &self.file else { return };
        let mut all: Vec<&Warp> = self.map.values().collect();
        all.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        let text: String = all.iter().map(|w| w.line() + "\n").collect();
        if let Err(e) = std::fs::write(f, text) {
            tracing::warn!("saving the warps failed: {e}");
        }
    }

    pub fn get(&self, name: &str) -> Option<&Warp> {
        self.map.get(&name.to_lowercase())
    }

    /// Adds a warp, replacing one of the same name.
    pub fn set(&mut self, w: Warp) {
        self.map.insert(w.id.to_lowercase(), w);
        self.save();
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let gone = self.map.remove(&name.to_lowercase()).is_some();
        if gone {
            self.save();
        }
        gone
    }

    /// Newest first.
    pub fn newest_first(&self) -> Vec<&Warp> {
        let mut v: Vec<&Warp> = self.map.values().collect();
        v.sort_by(|a, b| b.created.cmp(&a.created).then(a.id.cmp(&b.id)));
        v
    }
}

impl Warp {
    fn line(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.created,
            self.pos[0],
            self.pos[1],
            self.pos[2],
            self.head[0],
            self.head[1],
            self.head[2],
            clean(&self.creator),
            clean(&self.id),
        )
    }

    fn parse(l: &str) -> Option<Warp> {
        let w: Vec<&str> = l.splitn(9, '\t').collect();
        if w.len() != 9 {
            return None;
        }
        Some(Warp {
            created: w[0].parse().ok()?,
            pos: [w[1].parse().ok()?, w[2].parse().ok()?, w[3].parse().ok()?],
            head: [w[4].parse().ok()?, w[5].parse().ok()?, w[6].parse().ok()?],
            creator: w[7].to_owned(),
            id: w[8].to_owned(),
        })
    }
}

// ---------------------------------------------------------------------
// The world, as the commands need it

/// What the commands ask of the world.
pub trait Host {
    /// The player with this exact name.
    fn find(&self, name: &str) -> Option<i32>;
    fn name(&self, id: i32) -> String;
    fn pose(&self, id: i32) -> Option<Pose>;
    /// Every player in the world.
    fn players(&self) -> Vec<i32>;
    /// Moves a player: its `ClientTeleport` and what others see.
    fn move_to(&self, id: i32, mv: &Move);
    /// A chat line to one player.
    fn say(&self, id: i32, m: Msg);
    /// A notification (the pop-up at the side of the screen).
    fn notify(&self, id: i32, m: Msg);
    /// The highest block that is not see-through in a column.
    fn top(&self, x: i32, z: i32) -> Option<i32>;
}

/// The words of a command line: positional ones, `--name=value` options
/// (a flag has no value).
struct Args<'a> {
    pos: Vec<&'a str>,
    opts: Vec<(&'a str, &'a str)>,
}

impl<'a> Args<'a> {
    fn new(words: &[&'a str]) -> Args<'a> {
        let mut a = Args {
            pos: Vec::new(),
            opts: Vec::new(),
        };
        for w in words {
            match w.strip_prefix("--") {
                Some(o) => {
                    let (k, v) = o.split_once('=').unwrap_or((o, ""));
                    a.opts.push((k, v));
                }
                None => a.pos.push(w),
            }
        }
        a
    }

    fn opt(&self, name: &str) -> Option<&'a str> {
        self.opts.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
    }

    fn int(&self, name: &str) -> Option<i32> {
        self.opt(name).and_then(|v| v.parse().ok())
    }

    fn word(&self, i: usize) -> &'a str {
        self.pos.get(i).copied().unwrap_or("")
    }
}

/// Everything the teleport commands keep: each player's history and the
/// warps.
#[derive(Default)]
pub struct State {
    history: Mutex<HashMap<i32, History>>,
    warps: Mutex<Warps>,
}

fn red(m: Msg) -> Msg {
    m.color("#ff0000")
}

impl State {
    /// Keeps the warps in `file` (next to the world's blocks).
    pub fn open(&self, file: PathBuf) {
        *self.warps.lock().unwrap() = Warps::open(file);
    }

    /// A player left: its history goes with it.
    pub fn forget(&self, id: i32) {
        self.history.lock().unwrap().remove(&id);
    }

    fn remember(&self, id: i32, pos: [f64; 3], head: [f32; 3], message: &str) {
        self.history
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .append(pos, head, message);
    }

    /// Runs a valid command: `words[0]` is `tp`, `spawn` or `warp`.
    pub fn run(&self, h: &dyn Host, me: i32, words: &[&str]) {
        let Some((cmd, rest)) = words.split_first() else {
            return;
        };
        let a = Args::new(rest);
        match *cmd {
            "tp" => self.tp(h, me, &a),
            "spawn" => self.spawn(h, me, &a),
            "warp" => self.warp(h, me, &a),
            _ => {}
        }
    }

    // -----------------------------------------------------------------
    // /tp

    fn tp(&self, h: &dyn Host, me: i32, a: &Args) {
        match a.word(0) {
            "all" => self.tp_all(h, me, a),
            "home" => self.tp_home(h, me),
            "top" => self.tp_top(h, me),
            "back" => self.tp_back(h, me, a, false),
            "forward" => self.tp_back(h, me, a, true),
            "history" => self.tp_history(h, me),
            "world" => self.tp_world(h, me, a),
            _ => match a.pos.len() {
                1 => self.tp_to_player(h, me, a),
                2 => self.tp_other_to_player(h, me, a),
                3 => self.tp_to_coords(h, me, me, a, 0),
                4 => match h.find(a.word(0)) {
                    Some(t) => self.tp_to_coords(h, me, t, a, 1),
                    None => h.say(me, Msg::key("server.commands.errors.playerNotInWorld")),
                },
                _ => {}
            },
        }
    }

    fn tp_to_player(&self, h: &dyn Host, me: i32, a: &Args) {
        let Some(target) = h.find(a.word(0)) else {
            return h.say(me, Msg::key("server.commands.errors.targetNotInWorld"));
        };
        let (Some(mine), Some(theirs)) = (h.pose(me), h.pose(target)) else {
            return;
        };
        let name = h.name(target);
        h.move_to(me, &Move::facing(theirs.pos, theirs.head));
        h.say(
            me,
            Msg::key("server.commands.teleport.teleportedToPlayer").text("toName", &name),
        );
        self.remember(me, mine.pos, mine.head, &format!("Teleport to {name}"));
    }

    fn tp_other_to_player(&self, h: &dyn Host, me: i32, a: &Args) {
        let (Some(who), Some(target)) = (h.find(a.word(0)), h.find(a.word(1))) else {
            return;
        };
        let (Some(from), Some(to)) = (h.pose(who), h.pose(target)) else {
            return;
        };
        let (who_name, to_name) = (h.name(who), h.name(target));
        h.move_to(who, &Move::facing(to.pos, to.head));
        h.say(
            me,
            Msg::key("server.commands.teleport.teleportedOtherToPlayer")
                .text("targetName", &who_name)
                .text("toName", &to_name),
        );
        self.remember(
            who,
            from.pos,
            from.head,
            &format!("Teleport to {to_name} by {}", h.name(me)),
        );
    }

    /// Where `x y z` (words `at..at+3`) are for a player at `now`.
    fn place(h: &dyn Host, a: &Args, at: usize, now: &Pose) -> Result<[f64; 3], Msg> {
        let c = |i: usize| Coord::parse(a.word(at + i)).unwrap_or(Coord {
            value: 0.0,
            height: false,
            relative: false,
            chunk: false,
        });
        let (cx, cy, cz) = (c(0), c(1), c(2));
        let x = cx.xz(now.pos[0]);
        let z = cz.xz(now.pos[2]);
        let y = if cy.height {
            match h.top(x.floor() as i32, z.floor() as i32) {
                Some(top) => f64::from(top) + 1.0 + cy.resolve(0.0),
                None => return Err(Msg::raw("No ground found")),
            }
        } else {
            cy.resolve(now.pos[1])
        };
        Ok([x, y, z])
    }

    /// `/tp x y z` for `me` (`who == me`), or `/tp player x y z`.
    fn tp_to_coords(&self, h: &dyn Host, me: i32, who: i32, a: &Args, at: usize) {
        let Some(now) = h.pose(who) else { return };
        let pos = match Self::place(h, a, at, &now) {
            Ok(p) => p,
            Err(m) => return h.say(me, m),
        };
        let look = Look::from(a);
        let (yaw, pitch, roll) = look.radians(now.head);
        let mv = Move {
            pos,
            body: [now.body[0], yaw, now.body[2]],
            head: [pitch, yaw, roll],
        };
        let msg = {
            let m = if look.any() {
                Msg::key("server.commands.teleport.teleportedToCoordinatesWithLook")
                    .double("x", pos[0])
                    .double("y", pos[1])
                    .double("z", pos[2])
                    .double("yaw", degrees(yaw, now.head[1]))
                    .double("pitch", degrees(pitch, now.head[0]))
                    .double("roll", degrees(roll, now.head[2]))
            } else {
                Msg::key("server.commands.teleport.teleportedToCoordinates")
                    .double("x", pos[0])
                    .double("y", pos[1])
                    .double("z", pos[2])
            };
            m
        };
        let text = format!(
            "Teleport to ({}, {}, {})",
            java_string(pos[0]),
            java_string(pos[1]),
            java_string(pos[2])
        );
        h.move_to(who, &mv);
        if who == me {
            h.say(me, msg);
            self.remember(who, now.pos, now.head, &text);
        } else {
            self.remember(who, now.pos, now.head, &text);
            h.say(me, msg);
        }
    }

    fn tp_all(&self, h: &dyn Host, me: i32, a: &Args) {
        let Some(base) = h.pose(me) else { return };
        let pos = match Self::place(h, a, 1, &base) {
            Ok(p) => p,
            Err(m) => return h.say(me, m),
        };
        let look = Look::from(a);
        let sender = h.name(me);
        let text = format!(
            "Teleport to ({}, {}, {}) by {sender}",
            java_string(pos[0]),
            java_string(pos[1]),
            java_string(pos[2])
        );
        for id in h.players() {
            let Some(now) = h.pose(id) else { continue };
            let (yaw, pitch, roll) = look.radians(now.head);
            h.move_to(
                id,
                &Move {
                    pos,
                    body: [now.body[0], yaw, now.body[2]],
                    head: [pitch, yaw, roll],
                },
            );
            self.remember(id, now.pos, now.head, &text);
            let n = if look.any() {
                Msg::key("server.commands.teleport.teleportedWithLookNotification")
                    .double("x", pos[0])
                    .double("y", pos[1])
                    .double("z", pos[2])
                    .double("yaw", degrees(yaw, now.head[1]))
                    .double("pitch", degrees(pitch, now.head[0]))
                    .double("roll", degrees(roll, now.head[2]))
                    .text("sender", &sender)
            } else {
                Msg::key("server.commands.teleport.teleportedToCoordinatesNotification")
                    .double("x", pos[0])
                    .double("y", pos[1])
                    .double("z", pos[2])
                    .text("sender", &sender)
            };
            h.notify(id, n);
        }
        let raw = |n: &str| a.opt(n).and_then(|v| RelFloat::parse(v).ok()).map_or(0.0, |r| r.value);
        let m = if look.any() {
            Msg::key("server.commands.teleport.teleportEveryoneWithLook")
                .text("world", WORLD)
                .double("x", pos[0])
                .double("y", pos[1])
                .double("z", pos[2])
                .double("yaw", f64::from(raw("yaw")))
                .double("pitch", f64::from(raw("pitch")))
                .double("roll", f64::from(raw("roll")))
        } else {
            Msg::key("server.commands.teleport.teleportEveryone")
                .text("world", WORLD)
                .double("x", pos[0])
                .double("y", pos[1])
                .double("z", pos[2])
        };
        h.say(me, m);
    }

    fn tp_home(&self, h: &dyn Host, me: i32) {
        let Some(now) = h.pose(me) else { return };
        self.remember(me, now.pos, now.head, "Home");
        h.move_to(me, &Move::facing(SPAWN, SPAWN_HEAD));
        h.say(me, Msg::key("server.commands.teleport.teleportedSelfHome"));
    }

    fn tp_top(&self, h: &dyn Host, me: i32) {
        let Some(now) = h.pose(me) else { return };
        let (bx, bz) = (now.pos[0].floor() as i32, now.pos[2].floor() as i32);
        let Some(y) = h.top(bx, bz) else {
            return h.say(
                me,
                Msg::key("server.commands.teleport.top.noSurface"),
            );
        };
        self.remember(me, now.pos, now.head, "Underground");
        h.move_to(
            me,
            &Move {
                pos: [now.pos[0], f64::from(y) + 2.0, now.pos[2]],
                body: [0.0, f32::NAN, 0.0],
                head: NAN3,
            },
        );
        h.say(me, Msg::key("server.commands.teleport.teleportedToTop"));
    }

    fn tp_back(&self, h: &dyn Host, me: i32, a: &Args, forward: bool) {
        let count = if a.opt("count").is_some() {
            a.int("count").unwrap_or(1)
        } else {
            1
        };
        if count <= 0 {
            return h.say(me, red(Msg::key("server.modules.command.runtimeError")));
        }
        let Some(now) = h.pose(me) else { return };
        let went = self
            .history
            .lock()
            .unwrap()
            .entry(me)
            .or_default()
            .go(forward, count as usize, (now.pos, now.head));
        match went {
            Went::NotFurther => h.say(
                me,
                Msg::key(if forward {
                    "server.commands.teleport.notFurther"
                } else {
                    "server.commands.teleport.notFurtherBack"
                }),
            ),
            Went::To(p, left, other) => {
                h.move_to(me, &Move::facing(p.pos, p.head));
                let (named, plain) = if forward {
                    (
                        "server.commands.teleport.teleportedForwardToWaypoint",
                        "server.commands.teleport.teleportedForwardToCoordinates",
                    )
                } else {
                    (
                        "server.commands.teleport.teleportedBackToWaypoint",
                        "server.commands.teleport.teleportedBackToCoordinates",
                    )
                };
                let m = if p.message.is_empty() {
                    Msg::key(plain)
                } else {
                    Msg::key(named).text("name", &p.message)
                };
                h.say(
                    me,
                    m.double("x", p.pos[0])
                        .double("y", p.pos[1])
                        .double("z", p.pos[2])
                        .int("remaining", left as i32)
                        .int("otherDirection", other as i32),
                );
            }
        }
    }

    fn tp_history(&self, h: &dyn Host, me: i32) {
        let (b, f) = self
            .history
            .lock()
            .unwrap()
            .entry(me)
            .or_default()
            .sizes();
        if b == 0 && f == 0 {
            h.say(me, Msg::key("server.commands.teleport.history.empty"));
        } else {
            h.say(
                me,
                Msg::key("server.commands.teleport.history.info")
                    .int("backCount", b as i32)
                    .int("forwardCount", f as i32),
            );
        }
    }

    fn tp_world(&self, h: &dyn Host, me: i32, a: &Args) {
        let who = match a.opt("player") {
            Some(n) => match h.find(n) {
                Some(id) => id,
                None => return,
            },
            None => me,
        };
        let Some(now) = h.pose(who) else { return };
        self.remember(who, now.pos, now.head, &format!("World {WORLD}"));
        h.say(
            me,
            Msg::key("server.commands.teleport.teleportedToWorld")
                .text("worldName", WORLD)
                .double("x", SPAWN[0])
                .double("y", SPAWN[1])
                .double("z", SPAWN[2]),
        );
        h.move_to(who, &Move::facing(SPAWN, SPAWN_HEAD));
    }

    // -----------------------------------------------------------------
    // /spawn

    fn spawn(&self, h: &dyn Host, me: i32, a: &Args) {
        let who = if a.pos.is_empty() {
            me
        } else {
            match h.find(a.word(0)) {
                Some(id) => id,
                None => return,
            }
        };
        let Some(now) = h.pose(who) else { return };
        // The world has one spawn point.
        if let Some(i) = a.int("spawnIndex")
            && i != 0
        {
            h.say(
                me,
                Msg::key("server.commands.spawn.indexNotFound").int("maxIndex", 0),
            );
            return h.say(
                me,
                red(Msg::key("server.commands.errors.spawnIndexOutOfRange")
                    .int("index", i)
                    .int("maxIndex", 0)),
            );
        }
        self.remember(who, now.pos, now.head, &format!("World {WORLD}'s spawn"));
        // The form with a player names it, even when it is the sender.
        let m = if a.pos.is_empty() {
            Msg::key("server.commands.spawn.teleported")
        } else {
            Msg::key("server.commands.spawn.teleportedOther").text("username", &h.name(who))
        };
        h.say(
            me,
            m.double("x", SPAWN[0])
                .double("y", SPAWN[1])
                .double("z", SPAWN[2]),
        );
        h.move_to(who, &Move::facing(SPAWN, SPAWN_HEAD));
    }

    // -----------------------------------------------------------------
    // /warp

    fn warp(&self, h: &dyn Host, me: i32, a: &Args) {
        match a.word(0) {
            "go" => self.warp_go(h, me, a.word(1)),
            "list" => self.warp_list(h, me, a),
            "set" => self.warp_set(h, me, a),
            "remove" => self.warp_remove(h, me, a),
            "reload" => {
                let n = self.warps.lock().unwrap().reload();
                h.say(
                    me,
                    Msg::key("server.commands.teleport.warp.reloaded").int("count", n as i32),
                );
            }
            name => self.warp_go(h, me, name),
        }
    }

    fn warp_go(&self, h: &dyn Host, me: i32, name: &str) {
        let warp = self.warps.lock().unwrap().get(name).cloned();
        let Some(w) = warp else {
            return h.say(
                me,
                Msg::key("server.commands.teleport.warp.unknownWarp").text("name", name),
            );
        };
        let Some(now) = h.pose(me) else { return };
        self.remember(me, now.pos, now.head, &format!("Warp '{name}'"));
        h.move_to(me, &Move::facing(w.pos, w.head));
        h.say(
            me,
            Msg::key("server.commands.teleport.warp.warpedTo").text("name", name),
        );
    }

    fn warp_set(&self, h: &dyn Host, me: i32, a: &Args) {
        let name = a.word(1);
        // The names of the sub-commands cannot be warps.
        if RESERVED.contains(&name.to_lowercase().as_str()) {
            return h.say(
                me,
                Msg::key("server.commands.teleport.warp.reservedKeyword"),
            );
        }
        let Some(now) = h.pose(me) else { return };
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        self.warps.lock().unwrap().set(Warp {
            id: name.to_owned(),
            creator: h.name(me),
            created,
            pos: now.pos,
            head: now.head,
        });
        h.say(
            me,
            Msg::key("server.commands.teleport.warp.setWarp").text("name", name),
        );
    }

    fn warp_remove(&self, h: &dyn Host, me: i32, a: &Args) {
        let name = a.word(1).to_lowercase();
        if self.warps.lock().unwrap().remove(&name) {
            h.say(
                me,
                Msg::key("server.commands.teleport.warp.removedWarp").text("name", &name),
            );
        } else {
            h.say(
                me,
                Msg::key("server.commands.teleport.warp.unknownWarp").text("name", &name),
            );
        }
    }

    fn warp_list(&self, h: &dyn Host, me: i32, a: &Args) {
        let warps = self.warps.lock().unwrap();
        let all = warps.newest_first();
        // Without a page a player gets a window (not done yet).
        let Some(page) = a.int("page") else {
            return h.say(
                me,
                Msg::raw("The warp window is not available in Aerie yet: use /warp list --page=1"),
            );
        };
        if all.is_empty() {
            return h.say(me, Msg::key("server.commands.teleport.warp.noWarps"));
        }
        if page < 1 {
            return h.say(
                me,
                Msg::key("server.commands.teleport.warp.pageNumTooLow").int("page", page),
            );
        }
        let pages = all.len().div_ceil(WARPS_PER_PAGE);
        if pages < page as usize {
            return h.say(me, Msg::key("server.commands.teleport.warp.pageNumTooHigh"));
        }
        h.say(
            me,
            Msg::key("server.commands.teleport.warp.listHeader")
                .int("page", page)
                .int("pages", pages as i32),
        );
        let start = (page as usize - 1) * WARPS_PER_PAGE;
        for (i, w) in all.iter().skip(start).take(WARPS_PER_PAGE).enumerate() {
            h.say(
                me,
                Msg::key("server.commands.teleport.warp.listEntry")
                    .int("index", (start + i + 1) as i32)
                    .text("name", &w.id)
                    .text("creator", &w.creator),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn coordinates() {
        let c = Coord::parse("~5").unwrap();
        assert_eq!(c.xz(10.0), 15.0);
        assert_eq!(Coord::parse("~").unwrap().xz(7.5), 7.5);
        assert_eq!(Coord::parse("c2").unwrap().xz(0.0), 64.0);
        assert!(Coord::parse("_").unwrap().height);
        assert!(Coord::parse("abc").is_none());
        assert!(Coord::parse("c~5").is_none());
        assert_eq!(Coord::parse("1.5e1").unwrap().xz(0.0), 15.0);
    }

    #[test]
    fn rotation_words() {
        assert_eq!(RelFloat::parse("~").unwrap().resolve(30.0), 30.0);
        assert_eq!(RelFloat::parse("~10").unwrap().resolve(30.0), 40.0);
        assert_eq!(RelFloat::parse("10").unwrap().resolve(30.0), 10.0);
        assert_eq!(RelFloat::parse("~abc"), Err("abc".to_owned()));
    }

    #[test]
    fn java_numbers_as_text() {
        assert_eq!(java_string(1725.0), "1725.0");
        assert_eq!(java_string(-0.5), "-0.5");
        assert_eq!(java_string(1.0e7), "1.0E7");
        assert_eq!(java_string(12345678.5), "1.23456785E7");
        assert_eq!(java_string(0.0001), "1.0E-4");
        assert_eq!(java_string(0.001), "0.001");
    }

    #[test]
    fn history_goes_back_and_forward() {
        let mut h = History::default();
        h.append([1.0, 0.0, 0.0], [0.0; 3], "a");
        h.append([2.0, 0.0, 0.0], [0.0; 3], "b");
        assert_eq!(h.sizes(), (2, 0));
        let w = h.go(false, 1, ([3.0, 0.0, 0.0], [0.0; 3]));
        match w {
            Went::To(p, left, other) => {
                assert_eq!((p.message.as_str(), left, other), ("b", 1, 1));
            }
            _ => panic!(),
        }
        // Two at once: the first is passed over.
        let w = h.go(true, 5, ([2.0, 0.0, 0.0], [0.0; 3]));
        assert!(matches!(w, Went::To(_, 0, 2)));
        assert_eq!(h.go(true, 1, ([0.0; 3], [0.0; 3])), Went::NotFurther);
        h.append([9.0, 0.0, 0.0], [0.0; 3], "c");
        assert_eq!(h.sizes().1, 0);
        for _ in 0..150 {
            h.append([0.0; 3], [0.0; 3], "x");
        }
        assert_eq!(h.sizes().0, HISTORY_MAX);
    }

    #[test]
    fn a_move_keeps_what_is_nan() {
        let from = Pose {
            pos: [1.0, 2.0, 3.0],
            body: [0.1, 0.2, 0.3],
            head: [0.4, 0.5, 0.6],
        };
        let m = Move {
            pos: [f64::NAN, 5.0, f64::NAN],
            body: [0.0, f32::NAN, 0.0],
            head: [f32::NAN, 1.0, f32::NAN],
        };
        let p = m.apply(&from);
        assert_eq!(p.pos, [1.0, 5.0, 3.0]);
        assert_eq!(p.body, [0.0, 0.2, 0.0]);
        assert_eq!(p.head, [0.4, 1.0, 0.6]);
    }

    #[test]
    fn warps_are_kept() {
        let dir = std::env::temp_dir().join(format!("aerie-warps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("warps.txt");
        let mut w = Warps::open(file.clone());
        w.set(Warp {
            id: "Home".into(),
            creator: "Ana".into(),
            created: 5,
            pos: [1.0, 2.0, 3.0],
            head: [0.0, 1.5, 0.0],
        });
        assert!(w.get("HOME").is_some());
        let w2 = Warps::open(file);
        assert_eq!(w2.get("home").unwrap().head[1], 1.5);
        std::fs::remove_dir_all(dir).ok();
    }

    /// A world of two players that writes down what it is told.
    struct Fake {
        poses: RefCell<HashMap<i32, Pose>>,
        said: RefCell<Vec<(i32, String)>>,
    }

    impl Fake {
        fn new() -> Fake {
            let p = Pose {
                pos: [0.0; 3],
                body: [0.0; 3],
                head: [0.0; 3],
            };
            Fake {
                poses: RefCell::new(HashMap::from([(1, p), (2, p)])),
                said: RefCell::new(Vec::new()),
            }
        }
    }

    fn key(m: &Msg) -> String {
        m.clone().into_inner().message_id.unwrap_or_default()
    }

    impl Host for Fake {
        fn find(&self, name: &str) -> Option<i32> {
            match name {
                "Ana" => Some(1),
                "Bia" => Some(2),
                _ => None,
            }
        }
        fn name(&self, id: i32) -> String {
            if id == 1 { "Ana" } else { "Bia" }.into()
        }
        fn pose(&self, id: i32) -> Option<Pose> {
            self.poses.borrow().get(&id).copied()
        }
        fn players(&self) -> Vec<i32> {
            vec![1, 2]
        }
        fn move_to(&self, id: i32, mv: &Move) {
            let mut p = self.poses.borrow_mut();
            let n = mv.apply(&p[&id]);
            p.insert(id, n);
        }
        fn say(&self, id: i32, m: Msg) {
            self.said.borrow_mut().push((id, key(&m)));
        }
        fn notify(&self, id: i32, m: Msg) {
            self.said.borrow_mut().push((id, format!("notify {}", key(&m))));
        }
        fn top(&self, _x: i32, _z: i32) -> Option<i32> {
            Some(60)
        }
    }

    fn run(s: &State, h: &Fake, me: i32, line: &str) -> Vec<(i32, String)> {
        h.said.borrow_mut().clear();
        let w: Vec<&str> = line.split_whitespace().collect();
        s.run(h, me, &w);
        h.said.borrow().clone()
    }

    #[test]
    fn tp_forms_and_history() {
        let s = State::default();
        let h = Fake::new();
        let said = run(&s, &h, 1, "tp 10 20 30");
        assert_eq!(said[0].1, "server.commands.teleport.teleportedToCoordinates");
        assert_eq!(h.pose(1).unwrap().pos, [10.0, 20.0, 30.0]);
        run(&s, &h, 1, "tp Bia");
        assert_eq!(h.pose(1).unwrap().pos, [0.0; 3]);
        let said = run(&s, &h, 1, "tp history");
        assert_eq!(said[0].1, "server.commands.teleport.history.info");
        let said = run(&s, &h, 1, "tp back");
        assert_eq!(
            said[0].1,
            "server.commands.teleport.teleportedBackToWaypoint"
        );
        assert_eq!(h.pose(1).unwrap().pos, [10.0, 20.0, 30.0]);
        // Bia to Ana, and the other forms.
        let said = run(&s, &h, 1, "tp Bia Ana");
        assert_eq!(said[0].1, "server.commands.teleport.teleportedOtherToPlayer");
        assert_eq!(h.pose(2).unwrap().pos, [10.0, 20.0, 30.0]);
        run(&s, &h, 1, "tp Bia ~1 ~ ~ --yaw=90");
        assert_eq!(h.pose(2).unwrap().pos, [11.0, 20.0, 30.0]);
        let said = run(&s, &h, 1, "tp all 5 5 5");
        assert_eq!(said.len(), 3);
        assert_eq!(h.pose(2).unwrap().pos, [5.0, 5.0, 5.0]);
        let said = run(&s, &h, 2, "tp back --count=0");
        assert_eq!(said[0].1, "server.modules.command.runtimeError");
    }

    #[test]
    fn warps_commands() {
        let s = State::default();
        let h = Fake::new();
        assert_eq!(run(&s, &h, 1, "warp list --page=1")[0].1, "server.commands.teleport.warp.noWarps");
        run(&s, &h, 1, "warp set Camp");
        assert_eq!(run(&s, &h, 1, "warp set list")[0].1, "server.commands.teleport.warp.reservedKeyword");
        run(&s, &h, 1, "tp 3 3 3");
        assert_eq!(run(&s, &h, 1, "warp camp")[0].1, "server.commands.teleport.warp.warpedTo");
        assert_eq!(h.pose(1).unwrap().pos, [0.0; 3]);
        assert_eq!(run(&s, &h, 1, "warp list --page=2")[0].1, "server.commands.teleport.warp.pageNumTooHigh");
        assert_eq!(run(&s, &h, 1, "warp remove CAMP")[0].1, "server.commands.teleport.warp.removedWarp");
        assert_eq!(run(&s, &h, 1, "warp go camp")[0].1, "server.commands.teleport.warp.unknownWarp");
    }
}
