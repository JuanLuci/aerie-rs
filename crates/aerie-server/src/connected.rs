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
    /// Whether a block, turned so, supports what stands on top of it.
    fn supports_up(&self, block: i32, rot: i32) -> bool;
}

/// What a rule set wants at a cell: a block and rotation, and blocks at
/// offsets from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub block: i32,
    pub rot: i32,
    pub extra: Vec<([i32; 3], i32, i32)>,
}

const WEST: [i32; 3] = [-1, 0, 0];
const EAST: [i32; 3] = [1, 0, 0];

fn scaled(v: [i32; 3], n: i32) -> [i32; 3] {
    [v[0] * n, v[1] * n, v[2] * n]
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

/// A wide roof corner needs the cells beside it (toward the corner) to
/// hold the same block turned the same way, as their own base cell or as
/// this block's part.
#[allow(clippy::too_many_arguments)]
fn width_fulfilled(
    w: &dyn Cells,
    at: Pos,
    right: bool,
    yaw: i32,
    block: i32,
    rot: i32,
    width: i32,
) -> bool {
    for i in 0..width - 1 {
        let v = turn(yaw, scaled(if right { EAST } else { WEST }, i + 1));
        let required = (v[0] & 31) | (v[2] & 31) << 5 | (v[1] & 31) << 10;
        let Some((other, other_rot, filler)) = w.at(add(at, v)) else {
            continue;
        };
        let same = other == block && other_rot == rot;
        if !(same && (filler == 0 || filler == required)) {
            return false;
        }
    }
    true
}

/// Whether a stair fits a valley (or, inverted, a ridge) seen from a
/// roof turned `rot`, a number of quarter turns round.
fn valley_compatible(rot: i32, other: Option<(Stair, i32)>, offset: i32, inverted: bool) -> bool {
    let Some((shape, other_rot)) = other else {
        return false;
    };
    let target = (rot % 4 + offset) % 4;
    let other_yaw = other_rot % 4;
    if other_rot / 4 % 4 != rot / 4 % 4 {
        return false;
    }
    let corner = matches!(shape, Stair::CornerLeft | Stair::CornerRight);
    let inverted_corner = matches!(
        shape,
        Stair::InvertedCornerLeft | Stair::InvertedCornerRight
    );
    if (inverted && corner) || (!inverted && inverted_corner) {
        return false;
    }
    let (right, left) = if inverted {
        (Stair::InvertedCornerRight, Stair::InvertedCornerLeft)
    } else {
        (Stair::CornerRight, Stair::CornerLeft)
    };
    other_yaw == target
        || (shape == right && other_yaw == (target + 1) % 4)
        || (shape == left && other_yaw == (target + 3) % 4)
}

/// The corner a roof makes with the roofs one level up (a valley) or,
/// reversed, one level down (a ridge end): left (false) or right.
#[allow(clippy::too_many_arguments)]
fn valley(
    w: &dyn Cells,
    material: Option<&str>,
    place: Pos,
    check: Pos,
    rot: i32,
    reverse: bool,
    block: i32,
    width: i32,
) -> Option<bool> {
    let yaw = rot % 4;
    let cell = |dir: [i32; 3]| stair_at(w, add(check, turn(yaw, scaled(dir, width))), material);
    let back = cell(if reverse { SOUTH } else { NORTH })?;
    if !valley_compatible(rot, Some(back), 0, reverse) {
        return None;
    }
    let left = valley_compatible(rot, cell(if reverse { EAST } else { WEST }), 1, reverse);
    let right = valley_compatible(rot, cell(if reverse { WEST } else { EAST }), 3, reverse);
    if left == right {
        return None;
    }
    width_fulfilled(w, place, right, yaw, block, rot, width).then_some(right)
}

/// The block a roof should be.
fn roof(
    w: &dyn Cells,
    r: &p::RoofConnectedBlockRuleSet,
    at: Pos,
    block: i32,
    rot: i32,
) -> Option<Want> {
    let regular = r.regular.as_ref()?;
    let material = r.material_name.as_deref();
    let width = r.width;
    let (yaw, upside_down) = facing(rot);
    let mut shape = Stair::Straight;
    if let Some(right) = inverted_corner_connection(w, material, at, yaw, upside_down)
        && width_fulfilled(w, at, right, yaw, block, rot, width)
    {
        shape = Stair::inverted_corner(right);
    }
    if let Some(right) = corner_connection(w, material, at, rot, yaw, upside_down, width)
        && width_fulfilled(w, at, right, yaw, block, rot, width)
    {
        shape = Stair::corner(right);
    }
    if shape == Stair::Straight
        && let Some(right) = valley(
            w,
            material,
            at,
            add(at, [0, 1, 0]),
            rot,
            false,
            block,
            width,
        )
    {
        shape = Stair::inverted_corner(right);
    }
    if shape == Stair::Straight
        && let Some(right) = valley(
            w,
            material,
            at,
            add(at, [0, -1, 0]),
            rot,
            true,
            block,
            width,
        )
    {
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
    let plain = |block: i32| Want {
        block,
        rot,
        extra: Vec::new(),
    };
    // A straight roof with roofs sloping away on all four sides below it
    // is the ridge's top piece.
    if r.topper_block_id != -1 && shape == Stair::Straight {
        let below = add(at, [0, -1, 0]);
        let level = rot - rot % 4;
        let sides = [(NORTH, 2), (SOUTH, 0), (EAST, 1), (WEST, 3)];
        if sides.iter().all(|(dir, off)| {
            valley_compatible(level, stair_at(w, add(below, *dir), material), *off, true)
        }) {
            return Some(plain(r.topper_block_id));
        }
    }
    // Over a block that does not support its top face the hollow shapes
    // are used.
    if let Some(hollow) = &r.hollow {
        let under = w.at(add(at, [0, -1, 0])).unwrap_or((0, 0, 0));
        if !w.supports_up(under.0, under.1) {
            return Some(plain(block_of(hollow, shape)));
        }
    }
    let mut want = plain(block_of(regular, shape));
    // A wide corner that changes side leaves the cell on its old side to
    // the new block.
    if width > 0
        && let Some(old) = shape_in(regular, block).filter(|s| *s != Stair::Straight)
    {
        let side = |s: Stair| match s {
            Stair::CornerLeft | Stair::InvertedCornerLeft => -(width - 1),
            Stair::CornerRight | Stair::InvertedCornerRight => width - 1,
            Stair::Straight => 0,
        };
        if side(shape) != side(old) {
            want.extra
                .push((turn(rot % 4, scaled(EAST, side(old))), want.block, rot));
        }
    }
    Some(want)
}

/// What a rule set wants at a cell. `None`: no opinion (also for the
/// rule sets not done yet).
pub fn desired(w: &dyn Cells, at: Pos, block: i32, rot: i32) -> Option<Want> {
    match w.rules(block)? {
        RuleSet::StairConnectedBlockRuleSet(s) => stair(w, s, at, rot).map(|(block, rot)| Want {
            block,
            rot,
            extra: Vec::new(),
        }),
        RuleSet::RoofConnectedBlockRuleSet(r) => roof(w, r, at, block, rot),
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
fn collect(w: &dyn Cells, origin: Pos, found: &mut Vec<(Pos, Want)>) {
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
                    && (want.block, want.rot) != (block, rot)
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
pub fn neighbours(w: &dyn Cells, at: Pos) -> Vec<(Pos, Want)> {
    let mut found = Vec::new();
    collect(w, at, &mut found);
    found
}
