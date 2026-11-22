//! Connected blocks (spec 23): stairs and their like pick the block that
//! fits their neighbours. The world is read through [`Cells`]; what comes
//! back is the list of cells to change.

use aerie_protocol::proto::{self as p, ConnectedBlockRuleSet as RuleSet};

pub type Pos = (i32, i32, i32);

/// The blocks around: a cell's block id, rotation index and filler, or
/// `None` where nothing is loaded; and each block's rule set.
pub trait Cells {
    fn at(&self, p: Pos) -> Option<(i32, i32, i32)>;
    fn rules(&self, block: i32) -> Option<&RuleSet>;
}

const NORTH: [i32; 3] = [0, 0, -1];
const SOUTH: [i32; 3] = [0, 0, 1];

/// A vector turned by a yaw (quarter turns), as the official server turns
/// block offsets.
fn turn(yaw: i32, v: [i32; 3]) -> [i32; 3] {
    match yaw.rem_euclid(4) {
        0 => v,
        1 => [v[2], v[1], -v[0]],
        2 => [-v[0], v[1], -v[2]],
        _ => [-v[2], v[1], v[0]],
    }
}

fn add(p: Pos, v: [i32; 3]) -> Pos {
    (p.0 + v[0], p.1 + v[1], p.2 + v[2])
}

/// The shape a stair block is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stair {
    Straight,
    CornerLeft,
    CornerRight,
    InvertedCornerLeft,
    InvertedCornerRight,
}

impl Stair {
    fn corner(right: bool) -> Stair {
        if right {
            Stair::CornerRight
        } else {
            Stair::CornerLeft
        }
    }
    fn inverted_corner(right: bool) -> Stair {
        if right {
            Stair::InvertedCornerRight
        } else {
            Stair::InvertedCornerLeft
        }
    }
}

/// The stair shapes of a rule set by block id, and its material.
fn stair_set(r: &RuleSet) -> Vec<(&p::StairConnectedBlockRuleSet, Option<&str>)> {
    match r {
        RuleSet::StairConnectedBlockRuleSet(s) => vec![(s, s.material_name.as_deref())],
        RuleSet::RoofConnectedBlockRuleSet(r) => [&r.regular, &r.hollow]
            .into_iter()
            .flatten()
            .map(|s| (s, r.material_name.as_deref()))
            .collect(),
        _ => Vec::new(),
    }
}

fn shape_in(s: &p::StairConnectedBlockRuleSet, block: i32) -> Option<Stair> {
    [
        (s.straight_block_id, Stair::Straight),
        (s.corner_left_block_id, Stair::CornerLeft),
        (s.corner_right_block_id, Stair::CornerRight),
        (s.inverted_corner_left_block_id, Stair::InvertedCornerLeft),
        (s.inverted_corner_right_block_id, Stair::InvertedCornerRight),
    ]
    .into_iter()
    .find(|(id, _)| *id == block && *id != -1)
    .map(|(_, t)| t)
}

fn block_of(s: &p::StairConnectedBlockRuleSet, t: Stair) -> i32 {
    let id = match t {
        Stair::Straight => s.straight_block_id,
        Stair::CornerLeft => s.corner_left_block_id,
        Stair::CornerRight => s.corner_right_block_id,
        Stair::InvertedCornerLeft => s.inverted_corner_left_block_id,
        Stair::InvertedCornerRight => s.inverted_corner_right_block_id,
    };
    if id == -1 { s.straight_block_id } else { id }
}

/// The stair at a cell (its shape and rotation), when it is a stair-like
/// block of the material asked for, at its base cell.
fn stair_at(w: &dyn Cells, at: Pos, material: Option<&str>) -> Option<(Stair, i32)> {
    let (block, rot, filler) = w.at(at)?;
    if filler != 0 {
        return None;
    }
    let sets = stair_set(w.rules(block)?);
    let other = sets.first()?.1;
    if let (Some(a), Some(b)) = (material, other)
        && a != b
    {
        return None;
    }
    let shape = sets.iter().find_map(|(s, _)| shape_in(s, block))?;
    Some((shape, rot))
}

/// A stair's yaw as the rules see it and whether it is upside down.
fn facing(rot: i32) -> (i32, bool) {
    let upside_down = rot / 4 % 4 != 0;
    let yaw = rot % 4;
    (if upside_down { (yaw + 2) % 4 } else { yaw }, upside_down)
}

fn can_connect(yaw: i32, other: i32, upside_down: bool, other_upside_down: bool) -> bool {
    other_upside_down == upside_down && other != yaw && (other + 2) % 4 != yaw
}

/// Left (false) or right (true) corner with a stair turned a quarter.
fn connection(
    yaw: i32,
    other: i32,
    other_shape: Stair,
    inverted: bool,
    upside_down: bool,
) -> Option<bool> {
    if other == (yaw + 1) % 4
        && other_shape != Stair::inverted_corner(upside_down ^ inverted)
        && other_shape != Stair::corner(upside_down ^ !inverted)
    {
        Some(false)
    } else if other == (yaw + 3) % 4
        && other_shape != Stair::inverted_corner(upside_down ^ !inverted)
        && other_shape != Stair::corner(upside_down ^ inverted)
    {
        Some(true)
    } else {
        None
    }
}

/// The corner a stair makes with the stair behind it.
fn corner_connection(
    w: &dyn Cells,
    material: Option<&str>,
    at: Pos,
    rot: i32,
    yaw: i32,
    upside_down: bool,
    width: i32,
) -> Option<bool> {
    let behind = |n: i32| add(at, turn(yaw, [0, 0, NORTH[2] * n]));
    let mut back = stair_at(w, behind(width), material);
    if back.is_none() && width > 1 {
        back = stair_at(w, behind(width + 1), material).filter(|b| b.0 != Stair::Straight);
    }
    let (shape, other_rot) = back?;
    let (other_yaw, other_down) = facing(other_rot);
    if !can_connect(yaw, other_yaw, upside_down, other_down) {
        return None;
    }
    // Not when the stair beside it (in front of the other one) is turned
    // like this one: the two run on straight.
    let sideways = stair_at(w, add(at, turn(other_yaw, SOUTH)), material);
    if sideways.is_some_and(|s| s.1 == rot) {
        return None;
    }
    connection(yaw, other_yaw, shape, false, upside_down)
}

/// The inverted corner a stair makes with the stair in front of it.
fn inverted_corner_connection(
    w: &dyn Cells,
    material: Option<&str>,
    at: Pos,
    yaw: i32,
    upside_down: bool,
) -> Option<bool> {
    let (shape, other_rot) = stair_at(w, add(at, turn(yaw, SOUTH)), material)?;
    let (other_yaw, other_down) = facing(other_rot);
    if !can_connect(yaw, other_yaw, upside_down, other_down) {
        return None;
    }
    connection(yaw, other_yaw, shape, true, upside_down)
}

/// The block a stair should be: straight, or a corner with the stair in
/// front of it or (winning) behind it; mirrored when upside down.
fn stair(
    w: &dyn Cells,
    s: &p::StairConnectedBlockRuleSet,
    at: Pos,
    rot: i32,
) -> Option<(i32, i32)> {
    let material = s.material_name.as_deref();
    let (yaw, upside_down) = facing(rot);
    let mut shape = Stair::Straight;
    if let Some(right) = inverted_corner_connection(w, material, at, yaw, upside_down) {
        shape = Stair::inverted_corner(right);
    }
    if let Some(right) = corner_connection(w, material, at, rot, yaw, upside_down, 1) {
        shape = Stair::corner(right);
    }
    if upside_down {
        shape = match shape {
            Stair::CornerLeft => Stair::CornerRight,
            Stair::CornerRight => Stair::CornerLeft,
            Stair::InvertedCornerLeft => Stair::InvertedCornerRight,
            Stair::InvertedCornerRight => Stair::InvertedCornerLeft,
            s => s,
        };
    }
    Some((block_of(s, shape), rot))
}

/// What a rule set wants at a cell: block and rotation. `None`: no
/// opinion (also for the rule sets not done yet).
pub fn desired(w: &dyn Cells, at: Pos, block: i32, rot: i32) -> Option<(i32, i32)> {
    match w.rules(block)? {
        RuleSet::StairConnectedBlockRuleSet(s) => stair(w, s, at, rot),
        _ => None,
    }
}

/// Whether a rule set is only asked when its block is placed.
fn only_on_placement(r: &RuleSet) -> bool {
    !matches!(
        r,
        RuleSet::StairConnectedBlockRuleSet(_) | RuleSet::RoofConnectedBlockRuleSet(_)
    )
}

/// The changes the cells around `origin` want (spec 23): every
/// neighbour with a rule set, asked at its base cell.
fn collect(w: &dyn Cells, origin: Pos, found: &mut Vec<(Pos, (i32, i32))>) {
    for x in -1..=1 {
        for z in -1..=1 {
            for y in -1..=1 {
                if (x, y, z) == (0, 0, 0) {
                    continue;
                }
                let mut at = (origin.0 + x, origin.1 + y, origin.2 + z);
                if found.iter().any(|f| f.0 == at) {
                    continue;
                }
                let Some((block, rot, filler)) = w.at(at) else {
                    continue;
                };
                if w.rules(block).is_none_or(only_on_placement) {
                    continue;
                }
                if filler != 0 {
                    let sign = |n: i32| if n & 16 != 0 { n | -32 } else { n };
                    at = (
                        at.0 - sign(filler & 31),
                        at.1 - sign(filler >> 10 & 31),
                        at.2 - sign(filler >> 5 & 31),
                    );
                }
                if let Some(want) = desired(w, at, block, rot)
                    && want != (block, rot)
                    && !found.iter().any(|f| f.0 == at)
                {
                    found.push((at, want));
                }
            }
        }
    }
}

/// One round of neighbour updates at `at`: the cells that want to change
/// and what to. The caller applies them and asks again at each changed
/// cell, three steps deep.
pub fn neighbours(w: &dyn Cells, at: Pos) -> Vec<(Pos, (i32, i32))> {
    let mut found = Vec::new();
    collect(w, at, &mut found);
    found
}
