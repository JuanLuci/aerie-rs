//! Falling blocks (spec 24): a block that lost its support becomes an
//! entity that drops straight down and acts on the cell it lands in.

use crate::items::{self, Blocks};

const MASS: f64 = 0.5;
const DRAG: f64 = 0.05;
/// Below this squared step a body does not move at all.
const STILL: f64 = 1.0000000000000002e-10;

#[derive(Clone, Debug)]
pub struct Falling {
    pub block: i32,
    pub rot: i32,
    /// The centre of the cell it started in, then wherever it fell to.
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    /// Its block's bounding box around `pos`.
    pub bbox: [f64; 6],
    /// The hitbox collision config clients are told, -1 for none.
    pub collision: i32,
    pub announced: bool,
    /// Stopped against something that is not ground: it stays there.
    pub stuck: bool,
}

/// What one tick did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    pub moved: bool,
    pub landed: bool,
}

impl Falling {
    /// `bounds` is the block's bounding box in block space for its
    /// rotation.
    pub fn new(
        block: i32,
        rot: i32,
        cell: (i32, i32, i32),
        bounds: [f64; 6],
        collision: i32,
    ) -> Self {
        let mut bbox = bounds;
        for v in &mut bbox {
            *v -= 0.5;
        }
        Falling {
            block,
            rot,
            pos: [
                cell.0 as f64 + 0.5,
                cell.1 as f64 + 0.5,
                cell.2 as f64 + 0.5,
            ],
            vel: [0.0; 3],
            bbox,
            collision,
            announced: false,
            stuck: false,
        }
    }

    /// One tick: the move with the speed it had, then gravity.
    pub fn step(&mut self, dt: f32, w: &impl Blocks) -> Step {
        let mut out = Step::default();
        if self.stuck {
            self.vel = [0.0; 3];
            return out;
        }
        let d = dt as f64;
        let step = [self.vel[0] * d, self.vel[1] * d, self.vel[2] * d];
        if step.iter().map(|s| s * s).sum::<f64>() >= STILL {
            match items::sweep_box(self.pos, step, self.bbox, w) {
                Some(h) => {
                    out.moved = self.pos != h.point;
                    self.pos = h.point;
                    // Nothing bounces: it rests where it touched, and
                    // only a floor counts as landing.
                    self.vel = [0.0; 3];
                    if h.normal == [0.0, 1.0, 0.0] {
                        out.landed = true;
                    } else {
                        self.stuck = true;
                    }
                }
                None => {
                    for a in 0..3 {
                        self.pos[a] += step[a];
                    }
                    out.moved = true;
                }
            }
        } else {
            self.vel = [0.0; 3];
        }
        let area = ((self.bbox[3] - self.bbox[0]) * (self.bbox[5] - self.bbox[2])).abs();
        items::gravity_with(&mut self.vel, dt, MASS, DRAG, area);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Floor(i32);
    impl Blocks for Floor {
        fn solid_boxes(&self, _: i32, y: i32, _: i32) -> Option<Vec<[f64; 6]>> {
            (y == self.0).then(|| vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]])
        }
    }

    /// The first tick only gains speed; the fall ends with the box
    /// standing on the floor.
    #[test]
    fn falls_and_lands() {
        let mut f = Falling::new(1, 0, (5, 126, 7), BOX, -1);
        let w = Floor(121);
        let s = f.step(items::DT, &w);
        assert!(!s.moved && !s.landed);
        assert_eq!(f.pos[1], 126.5);
        let s = f.step(items::DT, &w);
        assert!(s.moved);
        let d = items::DT as f64;
        assert!((126.5 - f.pos[1] - 32.0 * d * d).abs() < 1.0e-9);
        let mut ticks = 2;
        while !f.step(items::DT, &w).landed {
            ticks += 1;
            assert!(ticks < 100);
        }
        assert_eq!(f.pos, [5.5, 122.5, 7.5]);
        assert_eq!(f.vel[0], 0.0);
    }

    const BOX: [f64; 6] = [0.1, 0.0, 0.1, 0.9, 1.0, 0.9];
}
