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
    /// The template a block fits itself by, and its shapes' blocks.
    fn template_use(&self, block: i32) -> Option<&Use>;
    fn template(&self, name: &str) -> Option<&Template>;
    /// How a block's yaw mirrors: 0 symmetric, 1 orthogonal, 2 orthogonal
    /// inverse.
    fn flip(&self, block: i32) -> u8;
    fn key(&self, block: i32) -> Option<&str>;
    /// A patterned rule set by its id.
    fn rule_set(&self, id: &str) -> Option<&p::ConnectedBlockRuleSetAsset>;
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
// ---- Templates (spec 23, "Templates") ----

/// Tags of a block's faces, by direction.
pub type Tags = Vec<([i32; 3], Vec<String>)>;

#[derive(Clone, Debug, Default)]
pub struct Template {
    pub default: Option<String>,
    /// Blocks of other materials count as neighbours.
    pub others: bool,
    /// Only asked when the block is placed.
    pub once: bool,
    /// In the order they are tried.
    pub shapes: Vec<Shape>,
}

#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub name: String,
    pub tags: Tags,
    pub patterns: Vec<Pattern>,
}

#[derive(Clone, Debug, Default)]
pub struct Pattern {
    /// The rules turn with the block.
    pub orient: bool,
    /// Extra yaw of the answer.
    pub yaw: i32,
    pub turns: bool,
    pub mirror_x: bool,
    pub mirror_z: bool,
    pub placement: bool,
    pub update: bool,
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, Default)]
pub struct Rule {
    pub pos: [i32; 3],
    pub include: bool,
    pub normals: Option<Vec<[i32; 3]>>,
    pub tags: Tags,
    /// (shape, rotation index, filler).
    pub shapes: Vec<(String, i32, i32)>,
    pub blocks: Vec<String>,
}

/// A block's use of a template: which template, and the blocks of each
/// shape (block id, rotation index).
#[derive(Clone, Debug, Default)]
pub struct Use {
    /// The block that declares it: blocks with the same owner are one
    /// material.
    pub owner: String,
    pub template: String,
    pub shapes: Vec<(String, Vec<(i32, i32)>)>,
}

fn vec3(v: Option<&serde_json::Value>) -> [i32; 3] {
    let n = |i: usize| {
        v.and_then(|v| v.get(i))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0) as i32
    };
    [n(0), n(1), n(2)]
}

fn tags_of(v: Option<&serde_json::Value>) -> Tags {
    v.and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(dir, tags)| {
            let d: Vec<i32> = dir.split(',').filter_map(|x| x.parse().ok()).collect();
            let [x, y, z] = d[..] else { return None };
            let tags = tags
                .as_array()?
                .iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect();
            Some(([x, y, z], tags))
        })
        .collect()
}

impl Template {
    /// From the asset side's table (the physics table's `CT` lines).
    pub fn from_json(v: &serde_json::Value) -> Self {
        use serde_json::Value;
        let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let list = |v: &Value, k: &str| -> Vec<Value> {
            v.get(k)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        Template {
            default: v.get("default").and_then(Value::as_str).map(str::to_owned),
            others: flag(v, "others"),
            once: flag(v, "once"),
            shapes: list(v, "shapes")
                .iter()
                .map(|s| Shape {
                    name: s
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    tags: tags_of(s.get("tags")),
                    patterns: list(s, "patterns")
                        .iter()
                        .map(|p| Pattern {
                            orient: flag(p, "orient"),
                            yaw: p.get("yaw").and_then(Value::as_i64).unwrap_or(0) as i32,
                            turns: flag(p, "turns"),
                            mirror_x: flag(p, "mirror_x"),
                            mirror_z: flag(p, "mirror_z"),
                            placement: flag(p, "placement"),
                            update: flag(p, "update"),
                            rules: list(p, "rules")
                                .iter()
                                .map(|r| Rule {
                                    pos: vec3(r.get("pos")),
                                    include: flag(r, "include"),
                                    normals: r
                                        .get("normals")
                                        .and_then(Value::as_array)
                                        .map(|n| n.iter().map(|x| vec3(Some(x))).collect()),
                                    tags: tags_of(r.get("tags")),
                                    shapes: list(r, "shapes")
                                        .iter()
                                        .filter_map(|e| {
                                            Some((
                                                e.get(0)?.as_str()?.to_owned(),
                                                e.get(1)?.as_i64()? as i32,
                                                e.get(2)?.as_i64()? as i32,
                                            ))
                                        })
                                        .collect(),
                                    blocks: list(r, "blocks")
                                        .iter()
                                        .filter_map(|b| b.as_str().map(str::to_owned))
                                        .collect(),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// A vector turned a quarter `n` times about x, as pitch turns blocks.
fn turn_x(n: i32, v: [i32; 3]) -> [i32; 3] {
    match n.rem_euclid(4) {
        0 => v,
        1 => [v[0], -v[2], v[1]],
        2 => [v[0], -v[1], -v[2]],
        _ => [v[0], v[2], -v[1]],
    }
}

/// The same about z (roll).
fn turn_z(n: i32, v: [i32; 3]) -> [i32; 3] {
    match n.rem_euclid(4) {
        0 => v,
        1 => [-v[1], v[0], v[2]],
        2 => [-v[0], -v[1], v[2]],
        _ => [v[1], -v[0], v[2]],
    }
}

fn unpack(filler: i32) -> [i32; 3] {
    let sign = |n: i32| if n & 16 != 0 { n | -32 } else { n };
    [
        sign(filler & 31),
        sign(filler >> 10 & 31),
        sign(filler >> 5 & 31),
    ]
}

fn pack(v: [i32; 3]) -> i32 {
    (v[0] & 31) | (v[2] & 31) << 5 | (v[1] & 31) << 10
}

/// A yaw mirrored across the x axis (`along_z` false) or the z axis, by
/// the block's flip type.
fn flip_yaw(flip: u8, yaw: i32, along_z: bool) -> i32 {
    let odd = yaw.rem_euclid(2) == 1;
    match flip {
        1 | 2 => {
            let mut step = if odd == (flip == 1) { 1 } else { -1 };
            if along_z {
                step = -step;
            }
            (yaw + step).rem_euclid(4)
        }
        _ => {
            // A yaw lies along z when even, along x when odd.
            if odd == along_z {
                yaw
            } else {
                (yaw + 2).rem_euclid(4)
            }
        }
    }
}

/// The shapes of a template use that a block id stands for.
fn shapes_for(u: &Use, block: i32) -> impl Iterator<Item = &str> {
    u.shapes
        .iter()
        .filter(move |(_, list)| list.iter().any(|(b, _)| *b == block))
        .map(|(n, _)| n.as_str())
}

/// One rule against the block found at its place, seen as turned back
/// (`rot`, `filler`).
fn rule_holds(
    w: &dyn Cells,
    placed: &Use,
    t: &Template,
    rule: &Rule,
    block: i32,
    rot: i32,
    filler: i32,
) -> bool {
    let same = |other: &Use| t.others || placed.owner == other.owner;
    if !rule.tags.is_empty() {
        let Some(u) = w.template_use(block) else {
            return !rule.include;
        };
        let Some(theirs) = w.template(&u.template) else {
            return !rule.include;
        };
        for name in shapes_for(u, block) {
            if !same(u) {
                continue;
            }
            let Some(shape) = theirs.shapes.iter().find(|s| s.name == name) else {
                continue;
            };
            for (dir, tags) in &rule.tags {
                let d = turn(-(rot % 4), *dir);
                let has = |tag: &String| {
                    shape
                        .tags
                        .iter()
                        .any(|(sd, st)| *sd == d && st.contains(tag))
                };
                if tags.iter().any(has) {
                    return rule.include;
                }
            }
        }
    }
    if !rule.shapes.is_empty() {
        let Some(u) = w.template_use(block) else {
            return !rule.include;
        };
        for name in shapes_for(u, block) {
            if same(u)
                && rule
                    .shapes
                    .iter()
                    .any(|(n, r, f)| n == name && *r == rot && *f == filler)
            {
                return rule.include;
            }
        }
    }
    if !rule.blocks.is_empty()
        && w.key(block)
            .is_some_and(|k| rule.blocks.iter().any(|b| b == k))
    {
        return rule.include;
    }
    !rule.include
}

/// One pattern at `at` for a block turned `rot`: the answer when its
/// rules hold in one of its transformations.
#[allow(clippy::too_many_arguments)]
fn pattern(
    w: &dyn Cells,
    u: &Use,
    t: &Template,
    shape: &str,
    p: &Pattern,
    at: Pos,
    rot: i32,
    normal: [i32; 3],
    placement: bool,
) -> Option<Want> {
    if (placement && p.update) || (!placement && p.placement) {
        return None;
    }
    // Plain, then mirrored in x, then in z; each as it is and, when the
    // pattern turns, by each quarter.
    let mut transforms = Vec::new();
    for (mirror, on) in [(0, true), (1, p.mirror_x), (2, p.mirror_z)] {
        if on {
            transforms.push((0, mirror));
            if p.turns {
                transforms.extend([(1, mirror), (2, mirror), (3, mirror)]);
            }
        }
    }
    'transform: for (yaw, mirror) in transforms {
        let mut total = [yaw, 0, 0];
        if p.orient {
            total = [(yaw + rot % 4) % 4, (rot / 4) % 4, (rot / 16) % 4];
        }
        for rule in &p.rules {
            let mut c = rule.pos;
            match mirror {
                1 => c[0] = -c[0],
                2 => c[2] = -c[2],
                _ => {}
            }
            if let Some(normals) = &rule.normals {
                if normals.contains(&normal) {
                    continue;
                }
                return None;
            }
            let c = turn(total[0], turn_x(total[1], turn_z(total[2], c)));
            let (block, their_rot, mut filler) = w.at(add(at, c))?;
            let mut back = [
                (their_rot % 4 - total[0]).rem_euclid(4),
                ((their_rot / 4) % 4 - total[1]).rem_euclid(4),
                ((their_rot / 16) % 4 - total[2]).rem_euclid(4),
            ];
            filler = pack(turn_x(-total[1], unpack(filler)));
            filler = pack(turn(-total[0], unpack(filler)));
            filler = pack(turn_z(-total[2], unpack(filler)));
            if mirror != 0 {
                let mirrored = flip_yaw(w.flip(block), back[0], mirror == 2);
                filler = pack(turn(mirrored - back[0], unpack(filler)));
                back[0] = mirrored;
            }
            let seen = back[2] * 16 + back[1] * 4 + back[0];
            if !rule_holds(w, u, t, rule, block, seen, filler) {
                continue 'transform;
            }
        }
        let (block, entry_rot) = *u.shapes.iter().find(|(n, _)| n == shape)?.1.first()?;
        let flip = w.flip(block);
        let mut r = [
            (entry_rot % 4 + p.yaw) % 4,
            (entry_rot / 4) % 4,
            (entry_rot / 16) % 4,
        ];
        if mirror != 0 {
            r[0] = flip_yaw(flip, r[0], mirror == 2);
        }
        for a in 0..3 {
            r[a] = (r[a] + total[a]) % 4;
        }
        if r[1] == 2 && flip == 1 {
            r[0] = (r[0] + 3) % 4;
        }
        return Some(Want {
            block,
            rot: r[2] * 16 + r[1] * 4 + r[0],
            extra: Vec::new(),
        });
    }
    None
}

/// What a templated block wants to be: the first shape with a pattern
/// that holds, else the template's default shape as the block is turned.
fn templated(
    w: &dyn Cells,
    u: &Use,
    at: Pos,
    rot: i32,
    normal: [i32; 3],
    placement: bool,
) -> Option<Want> {
    let t = w.template(&u.template)?;
    for shape in &t.shapes {
        for p in &shape.patterns {
            if let Some(want) = pattern(w, u, t, &shape.name, p, at, rot, normal, placement) {
                return Some(want);
            }
        }
    }
    let default = t.default.as_deref()?;
    let (block, _) = *u.shapes.iter().find(|(n, _)| n == default)?.1.first()?;
    Some(Want {
        block,
        rot,
        extra: Vec::new(),
    })
}

/// What the block at `at` wants to be. `normal` is the face it was
/// placed on, `placement` whether it is being placed now.
pub fn desired(
    w: &dyn Cells,
    at: Pos,
    block: i32,
    rot: i32,
    normal: [i32; 3],
    placement: bool,
) -> Option<Want> {
    if let Some(u) = w.template_use(block) {
        return templated(w, u, at, rot, normal, placement);
    }
    match w.rules(block)? {
        RuleSet::StairConnectedBlockRuleSet(s) => stair(w, s, at, rot).map(|(block, rot)| Want {
            block,
            rot,
            extra: Vec::new(),
        }),
        RuleSet::RoofConnectedBlockRuleSet(r) => roof(w, r, at, block, rot),
        RuleSet::PatternedConnectedBlockRuleSet(s) => patterned(w, s, at, block, rot),
    }
}

/// Whether a rule set is only asked when its block is placed.
fn only_on_placement(r: &RuleSet) -> bool {
    !matches!(
        r,
        RuleSet::StairConnectedBlockRuleSet(_)
            | RuleSet::RoofConnectedBlockRuleSet(_)
            | RuleSet::PatternedConnectedBlockRuleSet(_)
    )
}

// ---- Patterned rule sets (spec 23, "Patterned rule sets") ----

type Matrix = [[i32; 3]; 3];

fn cos90(q: i32) -> i32 {
    [1, 0, -1, 0][q.rem_euclid(4) as usize]
}

fn sin90(q: i32) -> i32 {
    [0, 1, 0, -1][q.rem_euclid(4) as usize]
}

/// A rotation index (roll × 16 + pitch × 4 + yaw) as a matrix.
fn matrix(rot: i32) -> Matrix {
    let (y, p, r) = (rot % 4, (rot / 4) % 4, (rot / 16) % 4);
    let (cy, sy, cp, sp, cr, sr) = (cos90(y), sin90(y), cos90(p), sin90(p), cos90(r), sin90(r));
    [
        [cy * cr + sy * sp * sr, -cy * sr + sy * sp * cr, sy * cp],
        [cp * sr, cp * cr, -sp],
        [-sy * cr + cy * sp * sr, sy * sr + cy * sp * cr, cy * cp],
    ]
}

fn times(a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = [[0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

fn transposed(a: &Matrix) -> Matrix {
    let mut m = [[0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[j][i];
        }
    }
    m
}

fn quarter_of(sin: i32, cos: i32) -> i32 {
    match (sin, cos) {
        (1, 0) => 1,
        (0, -1) => 2,
        (-1, 0) => 3,
        _ => 0,
    }
}

/// Back to a rotation index: pitch from the middle row, then yaw and
/// roll (yaw alone when the pitch is a quarter either way).
fn index_of(m: &Matrix) -> i32 {
    let sp = -m[1][2];
    let pitch = match sp {
        1 => 1,
        -1 => 3,
        _ => 0,
    };
    let (yaw, roll) = if sp != 1 && sp != -1 {
        (quarter_of(m[0][2], m[2][2]), quarter_of(m[1][0], m[1][1]))
    } else {
        (quarter_of(-m[2][0], m[0][0]), 0)
    };
    roll * 16 + pitch * 4 + yaw
}

/// `a` then `b`, as one rotation.
fn compose(a: i32, b: i32) -> i32 {
    if b == 0 {
        return a;
    }
    if a == 0 {
        return b;
    }
    index_of(&times(&matrix(a), &matrix(b)))
}

/// `b` seen from inside `a`.
fn compose_inverse(a: i32, b: i32) -> i32 {
    if a == 0 {
        return b;
    }
    index_of(&times(&transposed(&matrix(a)), &matrix(b)))
}

/// A vector turned by a rotation index: roll, pitch, then yaw.
fn apply(rot: i32, v: [i32; 3]) -> [i32; 3] {
    turn(rot % 4, turn_x((rot / 4) % 4, turn_z((rot / 16) % 4, v)))
}

/// The opposite: yaw back, pitch back, roll back.
fn undo(rot: i32, v: [i32; 3]) -> [i32; 3] {
    turn_z(
        -((rot / 16) % 4),
        turn_x(-((rot / 4) % 4), turn(-(rot % 4), v)),
    )
}

fn shape_tags(s: &p::ConnectedBlockShape) -> Option<&p::ConnectedBlockFaceTags> {
    match s {
        p::ConnectedBlockShape::TemplatedConnectedBlockShape(t) => t.face_tags.as_ref(),
        p::ConnectedBlockShape::BlockTypeConnectedBlockShape(t) => t.face_tags.as_ref(),
        p::ConnectedBlockShape::TagPatternConnectedBlockShape(t) => t.face_tags.as_ref(),
    }
}

fn shape_turn(s: &p::ConnectedBlockShape) -> i32 {
    match s {
        p::ConnectedBlockShape::TemplatedConnectedBlockShape(t) => t.relative_rotation_index,
        p::ConnectedBlockShape::BlockTypeConnectedBlockShape(t) => t.relative_rotation_index,
        p::ConnectedBlockShape::TagPatternConnectedBlockShape(t) => t.relative_rotation_index,
    }
}

fn tags_on(t: &p::ConnectedBlockFaceTags, d: [i32; 3]) -> Option<&Vec<String>> {
    match d {
        [0, 0, -1] => t.north.as_ref(),
        [1, 0, 0] => t.east.as_ref(),
        [0, 0, 1] => t.south.as_ref(),
        [-1, 0, 0] => t.west.as_ref(),
        [0, 1, 0] => t.up.as_ref(),
        [0, -1, 0] => t.down.as_ref(),
        _ => None,
    }
}

fn tag_dirs(t: &p::ConnectedBlockFaceTags) -> Vec<([i32; 3], &Vec<String>)> {
    [
        ([0, 0, -1], &t.north),
        ([1, 0, 0], &t.east),
        ([0, 0, 1], &t.south),
        ([-1, 0, 0], &t.west),
        ([0, 1, 0], &t.up),
        ([0, -1, 0], &t.down),
    ]
    .into_iter()
    .filter_map(|(d, v)| Some((d, v.as_ref()?)))
    .collect()
}

/// The shape name a block has in a patterned rule set's block map.
fn shape_in_map(set: &p::PatternedConnectedBlockRuleSet, block: i32) -> Option<&str> {
    set.shape_name_to_block_id
        .iter()
        .flatten()
        .find(|(_, b)| *b == block)
        .map(|(n, _)| n.as_str())
}

/// What a patterned rule set's rules see around a cell.
struct Context<'a> {
    w: &'a dyn Cells,
    set: &'a p::PatternedConnectedBlockRuleSet,
    asset: &'a p::ConnectedBlockRuleSetAsset,
    origin: Pos,
    rot: i32,
}

impl Context<'_> {
    /// The block and rotation at a rule's position, turned by the
    /// pattern's rotation; nothing for a part cell or an unloaded one.
    fn at(&self, v: [i32; 3]) -> Option<(i32, i32)> {
        let (block, rot, filler) = self.w.at(add(self.origin, apply(self.rot, v)))?;
        (filler == 0).then(|| (block, compose_inverse(self.rot, rot)))
    }

    /// The shape a block is: by its own patterned rule set when it has
    /// one, else by this one. (Its asset, the shape's name, the shape.)
    fn shape_of(&self, block: i32) -> Option<(&str, &str, &p::ConnectedBlockShape)> {
        let (set, asset) = match self.w.rules(block) {
            Some(RuleSet::PatternedConnectedBlockRuleSet(s)) => {
                match self.w.rule_set(&s.rule_set_id) {
                    Some(a) => (&**s, a),
                    None => (self.set, self.asset),
                }
            }
            _ => (self.set, self.asset),
        };
        for (name, shape) in asset.shapes.iter().flatten() {
            let fits = match shape {
                p::ConnectedBlockShape::TemplatedConnectedBlockShape(t) => {
                    shape_in_map(set, block).is_some()
                        && shape_in_map(set, block) == t.template_id.as_deref()
                }
                p::ConnectedBlockShape::BlockTypeConnectedBlockShape(t) => t.block_id == block,
                p::ConnectedBlockShape::TagPatternConnectedBlockShape(_) => false,
            };
            if fits {
                return Some((asset.id.as_str(), name.as_str(), shape));
            }
        }
        None
    }

    fn face(&self, offset: [i32; 3], dir: [i32; 3], tags: &[String]) -> bool {
        let look = [offset[0] + dir[0], offset[1] + dir[1], offset[2] + dir[2]];
        let Some((block, rot)) = self.at(look) else {
            return false;
        };
        let Some((_, _, shape)) = self.shape_of(block) else {
            return false;
        };
        let Some(theirs) = shape_tags(shape) else {
            return false;
        };
        let back = undo(shape_turn(shape), undo(rot, [-dir[0], -dir[1], -dir[2]]));
        let have = tags_on(theirs, back);
        !tags.is_empty() && tags.iter().all(|t| have.is_some_and(|h| h.contains(t)))
    }

    fn check(&self, rule: &p::ConnectedBlockRule) -> bool {
        use p::ConnectedBlockRule as R;
        match rule {
            R::AndConnectedBlockRule(a) => a.rules.iter().flatten().all(|r| self.check(r)),
            R::OrConnectedBlockRule(o) => o.rules.iter().flatten().any(|r| self.check(r)),
            R::NotConnectedBlockRule(n) => !self.check(&n.rule),
            R::ShapeConnectedBlockRule(s) => {
                let off = [
                    s.position_offset.x,
                    s.position_offset.y,
                    s.position_offset.z,
                ];
                let Some((block, rot)) = self.at(off) else {
                    return false;
                };
                let Some((asset, name, shape)) = self.shape_of(block) else {
                    return false;
                };
                if asset != self.asset.id || name != s.shape_id {
                    return false;
                }
                match &s.allowed_rotations {
                    Some(allowed) => allowed.contains(&compose(rot, shape_turn(shape))),
                    None => true,
                }
            }
            R::FaceTagConnectedBlockRule(f) => {
                let off = [
                    f.position_offset.x,
                    f.position_offset.y,
                    f.position_offset.z,
                ];
                let dirs = f.face_tags.as_ref().map(tag_dirs).unwrap_or_default();
                let any = f.match_type == p::ConnectedBlockMatchType::ANY;
                let mut all = true;
                for (dir, tags) in dirs {
                    let ok = self.face(off, dir, tags);
                    if ok && any {
                        return true;
                    }
                    all &= ok;
                }
                !any && all
            }
        }
    }
}

/// What a block of a patterned rule set wants to be: the first pattern
/// whose rule holds in one of its rotations gives its output shape,
/// turned by that rotation and the shape's own.
fn patterned(
    w: &dyn Cells,
    set: &p::PatternedConnectedBlockRuleSet,
    at: Pos,
    block: i32,
    rot: i32,
) -> Option<Want> {
    let asset = w.rule_set(&set.rule_set_id)?;
    let own = shape_in_map(set, block)?;
    asset.shapes.iter().flatten().find(|(n, _)| n == own)?;
    let output = |shape_id: &str, rot: i32| -> Option<Want> {
        let block = set
            .shape_name_to_block_id
            .iter()
            .flatten()
            .find(|(n, _)| n == shape_id)?
            .1;
        let shape = &asset
            .shapes
            .iter()
            .flatten()
            .find(|(n, _)| n == shape_id)?
            .1;
        Some(Want {
            block,
            rot: compose(rot, shape_turn(shape)),
            extra: Vec::new(),
        })
    };
    for pattern in asset.patterns.iter().flatten() {
        let Some(rule) = &pattern.rule else {
            return output(&pattern.output_shape_id, rot);
        };
        let turns = pattern
            .rotations
            .clone()
            .filter(|r| !r.is_empty())
            .unwrap_or(vec![0]);
        for t in turns {
            let composed = if pattern.transform_rules_with_orientation {
                compose(rot, t)
            } else {
                t
            };
            let ctx = Context {
                w,
                set,
                asset,
                origin: at,
                rot: composed,
            };
            if ctx.check(rule) {
                return output(&pattern.output_shape_id, composed);
            }
        }
    }
    None
}

/// A patterned rule set's update mode, for its block.
pub fn update_mode(w: &dyn Cells, block: i32) -> p::ConnectedBlockUpdateMode {
    match w.rules(block) {
        Some(RuleSet::PatternedConnectedBlockRuleSet(s)) => w
            .rule_set(&s.rule_set_id)
            .map_or(p::ConnectedBlockUpdateMode::PLACE_UPDATE, |a| a.update_mode),
        _ => p::ConnectedBlockUpdateMode::PLACE_UPDATE,
    }
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
                let once = match w.template_use(block) {
                    Some(u) => w.template(&u.template).is_none_or(|t| t.once),
                    None => w.rules(block).is_none_or(only_on_placement),
                };
                if once {
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
                if let Some(want) = desired(w, at, block, rot, [0, 0, 0], false)
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

#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn yaws_mirror_by_flip_type() {
        // Symmetric: half round when the yaw lies along the mirrored axis.
        assert_eq!(flip_yaw(0, 0, false), 0);
        assert_eq!(flip_yaw(0, 1, false), 3);
        assert_eq!(flip_yaw(0, 0, true), 2);
        assert_eq!(flip_yaw(0, 1, true), 1);
        // Orthogonal: a quarter forward for odd yaws, back for even ones;
        // the other way across z.
        assert_eq!(flip_yaw(1, 1, false), 2);
        assert_eq!(flip_yaw(1, 0, false), 3);
        assert_eq!(flip_yaw(1, 0, true), 1);
        // Orthogonal inverse: the opposite.
        assert_eq!(flip_yaw(2, 1, false), 0);
        assert_eq!(flip_yaw(2, 0, false), 1);
    }

    #[test]
    fn part_offsets_pack_like_the_world() {
        for v in [[-1, 0, 0], [0, 1, 0], [2, -3, 1]] {
            assert_eq!(unpack(pack(v)), v);
        }
        assert_eq!(pack([0, 1, 0]), 1024);
        assert_eq!(pack([0, 0, 1]), 32);
    }
}
