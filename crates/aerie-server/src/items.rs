//! Dropped items (spec 11): spawn, gravity, sweeping through blocks,
//! merging, pickup and the short flight to the player.

/// One world tick, in seconds, as the official server counts it.
pub const DT: f32 = 1.0 / 30.0;
pub const LAUNCH_SPEED: f64 = 3.25;
pub const SPREAD_SPEED: f64 = 3.0;
pub const MERGE_DELAY: f32 = 1.5;
pub const PICKUP_DELAY: f32 = 0.5;
pub const PICKUP_THROTTLE: f32 = 0.25;
pub const PICKUP_RADIUS: f64 = 1.75;
pub const MERGE_RADIUS: f64 = 2.0;
pub const FLIGHT_TIME: f32 = 0.15;
/// The item's box: 0.5 × 0.5 × 0.5, centred, bottom at the position.
pub const BOX: [f64; 6] = [-0.25, 0.0, -0.25, 0.25, 0.5, 0.25];
const MASS: f64 = 1.0;
const DRAG: f64 = 0.5;
const AIR_DENSITY: f64 = 0.001225;
const EPS: f64 = 1.0e-5;

/// Solid blocks as item physics sees them.
pub trait Blocks {
    /// The block's hitbox boxes (block-local, rotated) if it is solid.
    fn solid_boxes(&self, x: i32, y: i32, z: i32) -> Option<Vec<[f64; 6]>>;
}

#[derive(Clone, Debug)]
pub struct Item {
    pub item: String,
    pub quantity: i32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub merge_delay: f32,
    pub pickup_delay: f32,
    pub throttle: f32,
    /// Seconds left before it disappears.
    pub life: f32,
    /// The tick it was made in; it starts moving the tick after.
    pub born: u64,
    /// Clients know it.
    pub announced: bool,
    /// Its stack changed since clients last heard.
    pub stack_changed: bool,
}

impl Item {
    pub fn new(
        item: &str,
        quantity: i32,
        pos: [f64; 3],
        vel: [f64; 3],
        life: f32,
        born: u64,
    ) -> Self {
        Self {
            item: item.to_owned(),
            quantity,
            pos,
            vel,
            merge_delay: MERGE_DELAY,
            pickup_delay: PICKUP_DELAY,
            throttle: 0.0,
            life,
            born,
            announced: false,
            stack_changed: false,
        }
    }

    pub fn poll_pickup_delay(&mut self, dt: f32) -> bool {
        if self.pickup_delay <= 0.0 {
            return true;
        }
        self.pickup_delay -= dt;
        self.pickup_delay <= 0.0
    }

    pub fn poll_throttle(&mut self, dt: f32) -> bool {
        self.throttle -= dt;
        if self.throttle <= 0.0 {
            self.throttle = PICKUP_THROTTLE;
            true
        } else {
            false
        }
    }

    pub fn poll_merge_delay(&mut self, dt: f32) -> bool {
        self.merge_delay -= dt;
        if self.merge_delay <= 0.0 {
            self.merge_delay = MERGE_DELAY;
            true
        } else {
            false
        }
    }
}

/// Launch velocities for `n` stacks dropped together: straight up for
/// one, spread on a circle from `angle` for more.
pub fn launch(n: usize, angle: f32) -> Vec<[f64; 3]> {
    if n == 1 {
        return vec![[0.0, LAUNCH_SPEED, 0.0]];
    }
    (0..n)
        .map(|i| {
            let a = angle as f64 + std::f64::consts::TAU * i as f64 / n as f64;
            [SPREAD_SPEED * a.cos(), LAUNCH_SPEED, SPREAD_SPEED * a.sin()]
        })
        .collect()
}

fn signum(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Gravity for one tick (spec 11, step 2).
pub fn gravity(vel: &mut [f64; 3], dt: f32) {
    let area = (BOX[3] - BOX[0]) * (BOX[5] - BOX[2]);
    let terminal = -(64.0 * MASS * 1000.0 / (AIR_DENSITY * area * 1_000_000.0 * DRAG)).sqrt();
    let ratio = (vel[1] / terminal).abs();
    let accel = 32.0 * (signum(terminal) - signum(vel[1]) * (ratio * ratio * ratio));
    let step = accel * dt as f64;
    if vel[1] < terminal && step > 0.0 {
        vel[1] = (vel[1] + step).min(terminal);
    } else if vel[1] > terminal && step < 0.0 {
        vel[1] = (vel[1] + step).max(terminal);
    }
}

fn overlap(a: [f64; 6], pa: [f64; 3], b: [f64; 6], pb: [f64; 3]) -> bool {
    (0..3).all(|i| a[i] + pa[i] < b[i + 3] + pb[i] && b[i] + pb[i] < a[i + 3] + pa[i])
}

/// Out of a solid block (spec 11, step 1).
pub fn move_out(pos: &mut [f64; 3], vel: &mut [f64; 3], w: &impl Blocks) {
    let b = [
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ];
    let Some(boxes) = w.solid_boxes(b[0], b[1], b[2]) else {
        return;
    };
    let at = [b[0] as f64, b[1] as f64, b[2] as f64];
    if !boxes.iter().any(|bx| overlap(*bx, at, BOX, *pos)) {
        return;
    }
    let fx = pos[0] - at[0];
    let fz = pos[2] - at[2];
    let options = [
        ([-1, 0], fx),
        ([1, 0], 1.0 - fx),
        ([0, -1], fz),
        ([0, 1], 1.0 - fz),
    ];
    let mut best: Option<([i32; 2], f64)> = None;
    for (d, dist) in options {
        if w.solid_boxes(b[0] + d[0], b[1], b[2] + d[1]).is_none()
            && best.is_none_or(|(_, bd)| dist < bd)
        {
            best = Some((d, dist));
        }
    }
    match best {
        Some((d, _)) => {
            *pos = [
                (b[0] + d[0]) as f64 + 0.5,
                b[1] as f64,
                (b[2] + d[1]) as f64 + 0.5,
            ]
        }
        None => {
            let height = boxes.iter().map(|bx| bx[4]).fold(f64::MIN, f64::max)
                - boxes.iter().map(|bx| bx[1]).fold(f64::MAX, f64::min);
            vel[1] = 7.0 * height;
        }
    }
}

/// One axis of the sweep: (entry time, exit time, normal, kind) with kind
/// 0 outside, 1 inside, 2/3 touching the min/max side; `None` when it
/// cannot meet.
fn axis(p: f64, v: f64, min: f64, max: f64) -> Option<(f64, f64, f64, u8)> {
    let clamp_pos = |t: f64| if t < 0.0 { 0.0 } else { t };
    let outside = |border: f64, dist: f64, touch: u8, normal: f64| {
        if v != 0.0 {
            let mut enter = (dist / v).clamp(0.0, 1.0);
            if enter != 0.0 && enter < 1.0e-8 {
                enter = 0.0;
            }
            Some((enter, clamp_pos((border - p) / v), normal, 0))
        } else {
            Some((0.0, f64::MAX, normal, touch))
        }
    };
    let dist = min - p;
    if dist >= -EPS {
        if v < dist - EPS {
            return None;
        }
        return outside(max, dist, 2, -1.0);
    }
    let dist = max - p;
    if dist <= EPS {
        if v > dist + EPS {
            return None;
        }
        return outside(min, dist, 3, 1.0);
    }
    let (leave, normal) = if v < 0.0 {
        (clamp_pos((min - p) / v), 1.0)
    } else if v > 0.0 {
        (clamp_pos((max - p) / v), -1.0)
    } else {
        (f64::MAX, 0.0)
    };
    Some((0.0, leave, normal, 1))
}

/// A hit: when along the step (0–1), the face normal, the point reached.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f64,
    pub normal: [f64; 3],
    pub point: [f64; 3],
}

/// The item's box moving by `step` against one block box at `at`.
fn against(pos: [f64; 3], step: [f64; 3], block: [f64; 6], at: [f64; 3]) -> Option<Hit> {
    let mut axes = [(0.0, 0.0, 0.0, 0u8); 3];
    for a in 0..3 {
        axes[a] = axis(
            pos[a] - at[a],
            step[a],
            block[a] - BOX[a + 3],
            block[a + 3] - BOX[a],
        )?;
    }
    if axes.iter().all(|x| x.3 == 1) {
        // Overlapping already: not a hit for items.
        return None;
    }
    let mut start = -f64::MAX;
    let mut normal = [0.0; 3];
    for a in 0..3 {
        if axes[a].3 == 0 && axes[a].0 > start {
            start = axes[a].0;
            normal = [0.0; 3];
            normal[a] = axes[a].2;
        }
    }
    if start <= -f64::MAX {
        // Only touching: resting on a top face is not a hit.
        return None;
    }
    let end = axes.iter().map(|x| x.1).fold(f64::MAX, f64::min);
    if start > end {
        return None;
    }
    if axes[1].3 == 3 {
        return None;
    }
    if axes.iter().any(|x| x.3 >= 2) {
        return None;
    }
    Some(Hit {
        t: start,
        normal,
        point: [
            pos[0] + start * step[0],
            pos[1] + start * step[1],
            pos[2] + start * step[2],
        ],
    })
}

/// The first block hit moving the item from `pos` by `step`.
pub fn sweep(pos: [f64; 3], step: [f64; 3], w: &impl Blocks) -> Option<Hit> {
    let lo: Vec<i32> = (0..3)
        .map(|a| ((pos[a] + BOX[a]).min(pos[a] + step[a] + BOX[a]) - EPS).floor() as i32)
        .collect();
    let hi: Vec<i32> = (0..3)
        .map(|a| ((pos[a] + BOX[a + 3]).max(pos[a] + step[a] + BOX[a + 3]) + EPS).floor() as i32)
        .collect();
    let mut best: Option<Hit> = None;
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                let Some(boxes) = w.solid_boxes(x, y, z) else {
                    continue;
                };
                let at = [x as f64, y as f64, z as f64];
                for b in boxes {
                    if let Some(h) = against(pos, step, b, at)
                        && best.is_none_or(|bh| h.t < bh.t)
                    {
                        best = Some(h);
                    }
                }
            }
        }
    }
    best
}

/// Moves an item one tick (spec 11, step 3); `true` when its position
/// changed.
pub fn movement(pos: &mut [f64; 3], vel: &mut [f64; 3], dt: f32, w: &impl Blocks) -> bool {
    let dt = dt as f64;
    let step = [vel[0] * dt, vel[1] * dt, vel[2] * dt];
    if step.iter().map(|s| s * s).sum::<f64>() < 1.0000000000000002e-10 {
        return false;
    }
    match sweep(*pos, step, w) {
        Some(h) if h.normal == [0.0, 1.0, 0.0] => {
            *vel = [0.0; 3];
            let moved = *pos != h.point;
            *pos = h.point;
            moved
        }
        Some(h) => {
            let dot = vel[0] * h.normal[0] + vel[1] * h.normal[1] + vel[2] * h.normal[2];
            for a in 0..3 {
                vel[a] -= h.normal[a] * dot;
            }
            false
        }
        None => {
            for a in 0..3 {
                pos[a] += step[a];
            }
            true
        }
    }
}

/// The copy of a picked-up item flying to the player.
#[derive(Clone, Debug)]
pub struct Flight {
    pub item: String,
    pub quantity: i32,
    pub start: [f64; 3],
    pub pos: [f64; 3],
    pub to_player: i32,
    pub life: f32,
    pub announced: bool,
    pub finished: bool,
}

impl Flight {
    /// One tick towards `target`; `true` once it arrived.
    pub fn step(&mut self, target: [f64; 3], dt: f32) -> bool {
        let progress = 1.0 - self.life / FLIGHT_TIME;
        if progress >= 1.0 {
            self.pos = target;
            return true;
        }
        let p = progress as f64;
        for a in 0..3 {
            self.pos[a] = self.start[a] + (target[a] - self.start[a]) * p;
        }
        self.life -= dt;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ground(i32);
    impl Blocks for Ground {
        fn solid_boxes(&self, _x: i32, y: i32, _z: i32) -> Option<Vec<[f64; 6]>> {
            (y <= self.0).then(|| vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]])
        }
    }

    /// The official trunk drop of spec 11: positions tick by tick.
    #[test]
    fn rises_and_falls_like_the_capture() {
        let w = Ground(129);
        let mut pos = [530.5, 141.0, 254.5];
        let mut vel = [0.0, LAUNCH_SPEED, 0.0];
        let mut ys = Vec::new();
        for _ in 0..40 {
            move_out(&mut pos, &mut vel, &w);
            gravity(&mut vel, DT);
            movement(&mut pos, &mut vel, DT, &w);
            ys.push(pos[1]);
        }
        // The first ticks of the capture (later ones depend on the official
        // server's measured tick lengths, which vary).
        let want = [141.0726, 141.1097, 141.1111, 141.0771, 141.0074];
        for (g, w) in ys.iter().zip(want) {
            assert!((g - w).abs() < 1.0e-4, "{ys:?}");
        }
        // Lands on the ground's top face and stays.
        assert_eq!(*ys.last().unwrap(), 130.0);
        assert_eq!(vel, [0.0; 3]);
    }

    #[test]
    fn flight_reaches_the_player() {
        let mut f = Flight {
            item: "A".into(),
            quantity: 1,
            start: [0.0; 3],
            pos: [0.0; 3],
            to_player: 1,
            life: FLIGHT_TIME,
            announced: false,
            finished: false,
        };
        let mut n = 0;
        while !f.step([3.0, 0.0, 0.0], DT) {
            n += 1;
        }
        assert_eq!(f.pos, [3.0, 0.0, 0.0]);
        assert!(n >= 4);
    }
}
