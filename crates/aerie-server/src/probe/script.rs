//! Probe scripts: one step per line, run in order once the player is in
//! the world. Waits are on events (a chat reply, a window, a block change)
//! instead of fixed pauses, actions are written by name and built from the
//! server's own registries, and `state` / `expect` steps record what the
//! player looks like at that point. The step list is in `docs/probe.md`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aerie_protocol::{frame, proto, wire::Wire};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use super::state::{Event, State, id, section_id};

/// How long a wait or a command waits by default, in seconds.
const WAIT: f64 = 5.0;

#[derive(Clone, Debug)]
pub enum Wait {
    Chat(Option<String>),
    Window,
    Close,
    Block([i32; 3]),
    Packet(u32),
    Teleport,
    Inventory,
}

#[derive(Clone, Debug)]
pub enum Expect {
    Block([i32; 3], String),
    Item {
        section: i32,
        slot: i32,
        item: Option<String>,
        quantity: Option<i32>,
    },
    Window(bool),
    Mode(String),
    Chat(String),
    Held(Option<String>),
}

#[derive(Clone, Debug)]
pub enum Step {
    Sleep(f64),
    Chat(String),
    /// A chat line, then a wait for the server's reply.
    Command(String),
    Goto([f64; 3]),
    Look(f32, f32),
    Slot(i32),
    Use([i32; 3], i32),
    Act(i32, i32),
    Move {
        from: (i32, i32),
        quantity: i32,
        to: (i32, i32),
    },
    Close(Option<i32>),
    Send(Vec<(u32, Vec<u8>)>),
    Replay {
        dir: PathBuf,
        at: Option<[i32; 3]>,
        roots: Option<PathBuf>,
    },
    Wait(Wait, f64),
    State(String),
    Expect(Expect),
}

pub struct Line {
    pub number: usize,
    pub text: String,
    pub step: Step,
}

fn nums<T: std::str::FromStr>(w: &[&str], n: usize, what: &str) -> Result<Vec<T>> {
    if w.len() < n {
        bail!("{what}: needs {n} numbers");
    }
    w[..n]
        .iter()
        .map(|x| {
            x.parse()
                .ok()
                .with_context(|| format!("{what}: `{x}` is not a number"))
        })
        .collect()
}

/// A trailing `for SECS` on a wait.
fn timeout(w: &mut Vec<&str>) -> Result<f64> {
    if w.len() >= 2 && w[w.len() - 2] == "for" {
        let s = w[w.len() - 1].parse().context("`for` needs seconds")?;
        w.truncate(w.len() - 2);
        return Ok(s);
    }
    Ok(WAIT)
}

fn hex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("odd hex length");
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).context("bad hex"))
        .collect()
}

fn action_kind(s: &str) -> Result<i32> {
    Ok(match s {
        "take_all" => 0,
        "put_all" => 1,
        "quick_stack" => 2,
        "sort" => 3,
        n => n
            .parse()
            .context("act: kind is take_all, put_all, quick_stack, sort or a number")?,
    })
}

fn section(s: &str) -> Result<i32> {
    section_id(s)
        .with_context(|| format!("`{s}` is not a section (hotbar, storage, …, or a number)"))
}

pub fn parse_line(text: &str) -> Result<Option<Step>> {
    let text = text.split(" #").next().unwrap_or("").trim();
    if text.is_empty() || text.starts_with('#') {
        return Ok(None);
    }
    let (cmd, rest) = text.split_once(' ').unwrap_or((text, ""));
    let rest = rest.trim();
    let mut w: Vec<&str> = rest.split_whitespace().collect();
    let step = match cmd {
        "sleep" => Step::Sleep(nums(&w, 1, "sleep")?[0]),
        "chat" => Step::Chat(rest.to_owned()),
        "cmd" => Step::Command(rest.to_owned()),
        "goto" => {
            let v = nums(&w, 3, "goto")?;
            Step::Goto([v[0], v[1], v[2]])
        }
        "look" => {
            let v = nums(&w, 2, "look")?;
            Step::Look(v[0], v[1])
        }
        "slot" => Step::Slot(nums(&w, 1, "slot")?[0]),
        "use" => {
            let v = nums(&w, 3, "use")?;
            let face = match w.get(3) {
                Some(f) => face(f)?,
                None => 3,
            };
            Step::Use([v[0], v[1], v[2]], face)
        }
        "act" => {
            if w.len() < 2 {
                bail!("act SECTION KIND");
            }
            Step::Act(section(w[0])?, action_kind(w[1])?)
        }
        "move" => {
            if w.len() < 5 {
                bail!("move FROM_SECTION FROM_SLOT QUANTITY TO_SECTION TO_SLOT");
            }
            Step::Move {
                from: (section(w[0])?, w[1].parse()?),
                quantity: w[2].parse()?,
                to: (section(w[3])?, w[4].parse()?),
            }
        }
        "close" => Step::Close(w.first().map(|x| x.parse()).transpose()?),
        "send" => Step::Send(
            rest.split(',')
                .map(|p| {
                    let (i, h) = p.trim().split_once(':').context("send ID:HEX[,ID:HEX]")?;
                    Ok((i.parse()?, hex(h)?))
                })
                .collect::<Result<_>>()?,
        ),
        "replay" => {
            let dir = PathBuf::from(w.first().context("replay DIR [at X Y Z] [roots FILE]")?);
            let mut at = None;
            let mut roots = None;
            let mut i = 1;
            while i < w.len() {
                match w[i] {
                    "at" => {
                        let v = nums(&w[i + 1..], 3, "replay at")?;
                        at = Some([v[0], v[1], v[2]]);
                        i += 4;
                    }
                    "roots" => {
                        roots = Some(PathBuf::from(w.get(i + 1).context("roots FILE")?));
                        i += 2;
                    }
                    x => bail!("replay: `{x}` not known"),
                }
            }
            Step::Replay { dir, at, roots }
        }
        "wait" => {
            let secs = timeout(&mut w)?;
            let what = match w.first().copied() {
                Some("chat") => Wait::Chat((w.len() > 1).then(|| w[1..].join(" "))),
                Some("window") => Wait::Window,
                Some("close") => Wait::Close,
                Some("teleport") => Wait::Teleport,
                Some("inventory") => Wait::Inventory,
                Some("block") => {
                    let v = nums(&w[1..], 3, "wait block")?;
                    Wait::Block([v[0], v[1], v[2]])
                }
                Some("packet") => Wait::Packet(nums(&w[1..], 1, "wait packet")?[0]),
                _ => bail!(
                    "wait chat|window|close|teleport|inventory|block X Y Z|packet ID [for SECS]"
                ),
            };
            Step::Wait(what, secs)
        }
        "state" => Step::State(rest.to_owned()),
        "expect" => Step::Expect(match w.first().copied() {
            Some("block") => {
                let v = nums(&w[1..], 3, "expect block")?;
                Expect::Block(
                    [v[0], v[1], v[2]],
                    w.get(4).context("expect block X Y Z NAME")?.to_string(),
                )
            }
            Some("item") => {
                if w.len() < 4 {
                    bail!("expect item SECTION SLOT ITEM|empty [QUANTITY]");
                }
                Expect::Item {
                    section: section(w[1])?,
                    slot: w[2].parse()?,
                    item: (w[3] != "empty").then(|| w[3].to_owned()),
                    quantity: w.get(4).map(|q| q.parse()).transpose()?,
                }
            }
            Some("window") => Expect::Window(true),
            Some("nowindow") => Expect::Window(false),
            Some("mode") => Expect::Mode(w.get(1).context("expect mode NAME")?.to_string()),
            Some("chat") => Expect::Chat(w[1..].join(" ")),
            Some("held") => {
                Expect::Held(w.get(1).filter(|x| **x != "empty").map(|x| x.to_string()))
            }
            _ => bail!("expect block|item|window|nowindow|mode|chat|held …"),
        }),
        c => bail!("`{c}` is not a step"),
    };
    Ok(Some(step))
}

fn face(s: &str) -> Result<i32> {
    Ok(match s {
        "none" => 0,
        "up" => 1,
        "down" => 2,
        "north" => 3,
        "south" => 4,
        "east" => 5,
        "west" => 6,
        n => n
            .parse()
            .context("face: up, down, north, south, east, west or a number")?,
    })
}

pub fn parse(text: &str) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        match parse_line(l) {
            Ok(Some(step)) => out.push(Line {
                number: i + 1,
                text: l.trim().to_owned(),
                step,
            }),
            Ok(None) => {}
            Err(e) => bail!("script line {}: {e:#}", i + 1),
        }
    }
    Ok(out)
}

/// What a script run needs from the connection.
pub struct Run {
    pub state: Arc<Mutex<State>>,
    pub out: UnboundedSender<Vec<u8>>,
    pub capture: PathBuf,
}

fn packet<T: Wire>(id: u32, m: &T) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    m.encode(&mut payload)?;
    let mut w = Vec::new();
    frame::encode(id, &payload, &mut w)?;
    Ok(w)
}

impl Run {
    fn send(&self, w: Vec<u8>) -> Result<()> {
        self.out.send(w).ok().context("connection closed")
    }

    fn events(&self) -> usize {
        self.state.lock().unwrap().events.len()
    }

    /// Waits until an event after `mark` matches; false on time-out.
    async fn wait(&self, mark: usize, w: &Wait, secs: f64) -> bool {
        let end = tokio::time::Instant::now() + Duration::from_secs_f64(secs);
        loop {
            {
                let s = self.state.lock().unwrap();
                let hit = s.events[mark.min(s.events.len())..]
                    .iter()
                    .any(|e| match (w, e) {
                        (Wait::Chat(None), Event::Chat(_)) => true,
                        (Wait::Chat(Some(t)), Event::Chat(c)) => c.contains(t.as_str()),
                        (Wait::Window, Event::WindowOpened(_)) => true,
                        (Wait::Close, Event::WindowClosed(_)) => true,
                        (Wait::Block(p), Event::Block(b)) => p == b,
                        (Wait::Packet(i), Event::Packet(p)) => i == p,
                        (Wait::Teleport, Event::Teleport) => true,
                        (Wait::Inventory, Event::Inventory) => true,
                        _ => false,
                    });
                if hit || s.disconnect.is_some() {
                    return hit;
                }
            }
            if tokio::time::Instant::now() >= end {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Chains sent by hand or replayed take their ids: a server refuses a
    /// new chain whose id is not above the last one, so the probe's own
    /// chains continue after them.
    fn saw_chains(&self, m: &proto::SyncInteractionChains) {
        let top = m.updates.iter().map(|c| c.chain_id).max();
        if let Some(top) = top {
            let mut s = self.state.lock().unwrap();
            s.next_chain = s.next_chain.max(top + 1);
        }
    }

    /// Waits until the block at `p` is known (its section can come a
    /// moment after the player moved).
    async fn known(&self, p: [i32; 3]) {
        let end = tokio::time::Instant::now() + Duration::from_secs_f64(WAIT);
        while self
            .state
            .lock()
            .unwrap()
            .world
            .get(p[0], p[1], p[2])
            .is_none()
            && tokio::time::Instant::now() < end
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Runs every step; returns the results of the `expect` steps.
    pub async fn run(&self, lines: Vec<Line>) -> Result<Vec<Value>> {
        let mut checks = Vec::new();
        let mut states = String::new();
        // Waits look at what came after the last action.
        let mut mark = self.events();
        for l in lines {
            info!("script {}: {}", l.number, l.text);
            if self.state.lock().unwrap().disconnect.is_some() {
                bail!("disconnected before line {}", l.number);
            }
            let acting = !matches!(
                l.step,
                Step::Wait(..) | Step::State(_) | Step::Expect(_) | Step::Sleep(_)
            );
            if acting {
                mark = self.events();
            }
            match &l.step {
                Step::Sleep(s) => tokio::time::sleep(Duration::from_secs_f64(*s)).await,
                Step::Chat(t) => self.send(packet(
                    id::CHAT_MESSAGE,
                    &proto::ChatMessage {
                        message: Some(t.clone()),
                    },
                )?)?,
                Step::Command(t) => {
                    self.send(packet(
                        id::CHAT_MESSAGE,
                        &proto::ChatMessage {
                            message: Some(t.clone()),
                        },
                    )?)?;
                    if !self.wait(mark, &Wait::Chat(None), WAIT).await {
                        warn!("line {}: no reply to `{t}`", l.number);
                    }
                }
                Step::Goto([x, y, z]) => {
                    self.send(packet(
                        id::CLIENT_MOVEMENT,
                        &proto::ClientMovement {
                            absolute_position: Some(proto::Position {
                                x: *x,
                                y: *y,
                                z: *z,
                            }),
                            ..Default::default()
                        },
                    )?)?;
                    self.state.lock().unwrap().position = Some([*x, *y, *z]);
                }
                Step::Look(yaw, pitch) => {
                    let d = proto::Direction {
                        yaw: *yaw,
                        pitch: *pitch,
                        roll: 0.0,
                    };
                    self.send(packet(
                        id::CLIENT_MOVEMENT,
                        &proto::ClientMovement {
                            look_orientation: Some(d.clone()),
                            body_orientation: Some(proto::Direction { pitch: 0.0, ..d }),
                            ..Default::default()
                        },
                    )?)?;
                    let mut s = self.state.lock().unwrap();
                    s.look = Some([*yaw, *pitch, 0.0]);
                    s.body = Some([*yaw, 0.0, 0.0]);
                }
                Step::Slot(n) => {
                    self.send(packet(
                        id::SET_ACTIVE_SLOT,
                        &proto::SetActiveSlot {
                            inventory_section_id: -1,
                            active_slot: *n,
                        },
                    )?)?;
                    self.state.lock().unwrap().active.insert(-1, *n);
                }
                Step::Use(pos, face) => {
                    self.known(*pos).await;
                    let m = use_chain(&mut self.state.lock().unwrap(), *pos, *face)?;
                    self.send(packet(id::INTERACTION_CHAINS, &m)?)?;
                }
                Step::Act(section, kind) => self.send(packet(
                    id::INVENTORY_ACTION,
                    &proto::InventoryAction {
                        inventory_section_id: *section,
                        inventory_action_type: proto::InventoryActionType(*kind),
                        action_data: 0,
                    },
                )?)?,
                Step::Move { from, quantity, to } => self.send(packet(
                    id::MOVE_ITEM_STACK,
                    &proto::MoveItemStack {
                        from_section_id: from.0,
                        from_slot_id: from.1,
                        quantity: *quantity,
                        to_section_id: to.0,
                        to_slot_id: to.1,
                    },
                )?)?,
                Step::Close(w) => {
                    let w = match w {
                        Some(w) => *w,
                        None => *self
                            .state
                            .lock()
                            .unwrap()
                            .windows
                            .keys()
                            .next_back()
                            .context("close: no window open")?,
                    };
                    self.send(packet(id::CLOSE_WINDOW, &proto::CloseWindow { id: w })?)?;
                }
                Step::Send(list) => {
                    let mut w = Vec::new();
                    for (i, b) in list {
                        if *i == id::INTERACTION_CHAINS
                            && let Ok((m, _)) = proto::SyncInteractionChains::decode(b, 0)
                        {
                            self.saw_chains(&m);
                        }
                        frame::encode(*i, b, &mut w)?;
                    }
                    self.send(w)?;
                }
                Step::Replay { dir, at, roots } => self.replay(dir, *at, roots.as_deref()).await?,
                Step::Wait(w, secs) => {
                    if !self.wait(mark, w, *secs).await {
                        warn!("line {}: `{}` timed out after {secs} s", l.number, l.text);
                        checks.push(json!({"line": l.number, "step": l.text, "ok": false, "got": "timed out"}));
                    }
                }
                Step::State(label) => {
                    let r = self.state.lock().unwrap().report();
                    let line = json!({"line": l.number, "label": label, "state": r});
                    states += &format!("{line}\n");
                    std::fs::write(self.capture.join("states.jsonl"), &states)?;
                }
                Step::Expect(e) => {
                    if let Expect::Block(p, _) = e {
                        self.known(*p).await;
                    }
                    let (ok, got) = check(&mut self.state.lock().unwrap(), e);
                    if ok {
                        info!("line {}: ok", l.number);
                    } else {
                        warn!("line {}: `{}` failed, got {got}", l.number, l.text);
                    }
                    checks.push(json!({"line": l.number, "step": l.text, "ok": ok, "got": got}));
                }
            }
        }
        Ok(checks)
    }

    /// Sends a recording's interaction chains again: from each chain's
    /// first packet, at the recorded pace, moved so the first block it
    /// names is `at` (by default the block under the player), with the
    /// held slot set to the probe's and root ids translated by name.
    async fn replay(&self, dir: &Path, at: Option<[i32; 3]>, roots: Option<&Path>) -> Result<()> {
        let mut list = super::recorded_chains(dir)?;
        let (target, slot, map) = {
            let s = self.state.lock().unwrap();
            let target = match at {
                Some(a) => a,
                None => {
                    let p = s.position.context("replay: position not known")?;
                    [
                        p[0].floor() as i32,
                        p[1].floor() as i32 - 1,
                        p[2].floor() as i32,
                    ]
                }
            };
            let slot = s.active.get(&-1).copied().unwrap_or(0);
            // `roots FILE`: the `roots.tsv` of the server the recording
            // was made on; ids are translated to this server's by name.
            let mut map = std::collections::HashMap::new();
            if let Some(f) = roots {
                for l in std::fs::read_to_string(f)?.lines() {
                    if let Some((i, name)) = l.split_once('\t')
                        && let (Ok(i), Some(n)) = (i.parse::<i32>(), s.reg.root_by_name(name))
                    {
                        map.insert(i, n);
                    }
                }
            }
            (target, slot, map)
        };
        let first = list
            .iter()
            .flat_map(|m| &m.1.updates)
            .find_map(|c| c.data.as_ref()?.block_position.clone())
            .context("replay: the recording names no block")?;
        let d = (
            target[0] - first.x,
            target[1] - first.y,
            target[2] - first.z,
        );
        info!(
            "replaying {} packets from {} at {target:?}",
            list.len(),
            dir.display()
        );
        let mut last: Option<u64> = None;
        for (t, m) in &mut list {
            if let (Some(a), Some(l)) = (*t, last) {
                tokio::time::sleep(Duration::from_millis(a.saturating_sub(l))).await;
            }
            last = t.or(last);
            for c in &mut m.updates {
                super::shift(c, d);
                if !map.is_empty() {
                    super::remap_roots(c, &map);
                }
                if c.initial {
                    c.active_hotbar_slot = slot;
                }
            }
            self.saw_chains(m);
            self.send(packet(id::INTERACTION_CHAINS, &*m)?)?;
            if t.is_none() {
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
        }
        Ok(())
    }
}

/// A `Use` chain on a block, as a client sends it when the held item's use
/// hands over to the block's: the item's root finishes, fails into the
/// block's root, and the block's root finishes. Both roots come from the
/// registries this server sent.
pub fn use_chain(s: &mut State, pos: [i32; 3], face: i32) -> Result<proto::SyncInteractionChains> {
    const USE: i32 = 6;
    let held = s.held().map(|i| i.id.clone());
    let item_root = s
        .reg
        .item_root(held.as_deref(), USE)
        .or_else(|| s.reg.item_root(None, USE))
        .with_context(|| {
            format!(
                "use: {} has no use interaction",
                held.as_deref().unwrap_or("an empty hand")
            )
        })?;
    let block = s
        .world
        .get(pos[0], pos[1], pos[2])
        .with_context(|| format!("use: block at {pos:?} not known (its section was not sent)"))?;
    let block_root = s
        .reg
        .blocks
        .get(&block)
        .and_then(|b| b.interactions.iter().find(|x| x.0 == USE))
        .map(|x| x.1)
        .with_context(|| format!("use: {} has no use interaction", s.reg.block_name(block)))?;
    let chain_id = s.next_chain;
    s.next_chain += 1;
    let bp = proto::BlockPosition {
        x: pos[0],
        y: pos[1],
        z: pos[2],
    };
    let op = |state, counter, root, entered, at: Option<proto::BlockPosition>, face| {
        proto::InteractionSyncData {
            state,
            operation_counter: counter,
            root_interaction: root,
            entered_root_interaction: entered,
            block_position: at,
            block_face: proto::BlockFace(face),
            placed_block_id: i32::MIN,
            charge_value: -1.0,
            chaining_index: -1,
            flag_index: -1,
            ..Default::default()
        }
    };
    let chain = proto::SyncInteractionChain {
        active_hotbar_slot: s.active.get(&-1).copied().unwrap_or(0),
        active_utility_slot: -1,
        active_tools_slot: -1,
        item_in_hand_id: held,
        initial: true,
        override_root_interaction: i32::MIN,
        interaction_type: proto::InteractionType(USE),
        chain_id,
        data: Some(proto::InteractionChainData {
            entity_id: -1,
            block_position: Some(bp.clone()),
            target_slot: i32::MIN,
            ..Default::default()
        }),
        state: proto::InteractionState::FAILED,
        interaction_data: Some(vec![
            Some(op(
                proto::InteractionState::FINISHED,
                0,
                item_root,
                i32::MIN,
                None,
                0,
            )),
            Some(op(
                proto::InteractionState::FAILED,
                1,
                item_root,
                block_root,
                Some(bp),
                face,
            )),
            Some(op(
                proto::InteractionState::FINISHED,
                0,
                block_root,
                i32::MIN,
                None,
                0,
            )),
        ]),
        ..Default::default()
    };
    info!(
        "use {pos:?}: {} with {} ({} into {})",
        s.reg.block_name(block),
        chain.item_in_hand_id.as_deref().unwrap_or("an empty hand"),
        s.reg.root_name(item_root),
        s.reg.root_name(block_root)
    );
    Ok(proto::SyncInteractionChains {
        updates: vec![chain],
    })
}

fn check(s: &mut State, e: &Expect) -> (bool, Value) {
    match e {
        Expect::Block(p, name) => {
            let got = s.world.get(p[0], p[1], p[2]).map(|b| s.reg.block_name(b));
            (got.as_deref() == Some(name.as_str()), json!(got))
        }
        Expect::Item {
            section,
            slot,
            item,
            quantity,
        } => {
            let it = s
                .sections
                .get(section)
                .or_else(|| s.windows.get(section).and_then(|w| w.section.as_ref()))
                .and_then(|x| x.slots.get(slot));
            let ok = match (it, item) {
                (None, None) => true,
                (Some(i), Some(want)) => &i.id == want && quantity.is_none_or(|q| q == i.quantity),
                _ => false,
            };
            (ok, json!(it.map(|i| format!("{} x{}", i.id, i.quantity))))
        }
        Expect::Window(open) => (
            s.windows.is_empty() != *open,
            json!(s.windows.keys().collect::<Vec<_>>()),
        ),
        Expect::Mode(m) => {
            let got = match s.game_mode {
                Some(0) => "adventure".to_owned(),
                Some(1) => "creative".to_owned(),
                Some(n) => n.to_string(),
                None => "unknown".to_owned(),
            };
            (&got == m, json!(got))
        }
        Expect::Chat(t) => (
            s.chat.iter().any(|c| c.contains(t.as_str())),
            json!(s.chat.last()),
        ),
        Expect::Held(want) => {
            let got = s.held().map(|i| i.id.clone());
            (&got == want, json!(got))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_step() {
        let s = parse(
            "# setup\n\
             cmd /gamemode creative\n\
             chat hello there\n\
             goto 1.5 80 -3\n\
             look 0 -1.2  # down\n\
             slot 2\n\
             use 1 79 -3 up\n\
             act window1 sort\n\
             act -2 take_all\n\
             move hotbar 0 5 1 3\n\
             close\n\
             close 1\n\
             send 177:ffffffff02000000,202:01000000\n\
             replay rec at 1 2 3 roots r.tsv\n\
             wait window for 3\n\
             wait chat Hello\n\
             wait block 1 79 -3\n\
             wait packet 200\n\
             state after open\n\
             expect block 1 79 -3 Empty\n\
             expect item hotbar 0 empty\n\
             expect item storage 3 Rock_Stone 5\n\
             expect nowindow\n\
             expect mode creative\n\
             expect held Rock_Stone\n\
             sleep 0.5\n",
        )
        .unwrap();
        assert_eq!(s.len(), 25);
        assert!(matches!(s[5].step, Step::Use([1, 79, -3], 1)));
        assert!(matches!(s[7].step, Step::Act(-2, 0)));
        assert!(matches!(
            s[8].step,
            Step::Move {
                from: (-1, 0),
                quantity: 5,
                to: (1, 3)
            }
        ));
        assert!(matches!(s[13].step, Step::Wait(Wait::Window, t) if t == 3.0));
        assert!(matches!(&s[14].step, Step::Wait(Wait::Chat(Some(t)), _) if t == "Hello"));
        assert!(parse("jump").is_err());
        assert!(parse("goto 1 2").is_err());
    }
}
