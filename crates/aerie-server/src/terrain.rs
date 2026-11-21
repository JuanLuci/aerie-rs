//! Aerie's own terrain: rolling hills from value noise, grass over dirt over
//! stone over bedrock, and oak trees. Deterministic for a seed; flat around
//! the spawn so a player lands on the ground.

use crate::chunk::{SIZE, Section, index};

/// Block ids the generator uses (from the BlockTypes registry).
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bedrock: i32,
    pub stone: i32,
    pub dirt: i32,
    pub grass: i32,
    pub trunk: i32,
    pub leaves: i32,
}

impl Palette {
    /// The ids by key; `None` if the registry lacks one of them.
    pub fn new(id: impl Fn(&str) -> Option<i32>) -> Option<Self> {
        let first = |keys: &[&str]| keys.iter().find_map(|k| id(k));
        Some(Self {
            bedrock: first(&["Rock_Bedrock", "Rock_Stone"])?,
            stone: first(&["Rock_Stone"])?,
            dirt: first(&["Soil_Dirt"])?,
            grass: first(&["Soil_Grass"])?,
            trunk: first(&["Wood_Oak_Trunk"])?,
            leaves: first(&["Plant_Leaves_Oak", "Plant_Leaves_Apple", "Plant_Leaves_Ash"])?,
        })
    }
}

#[derive(Clone)]
pub struct Generator {
    pub seed: u64,
    pub palette: Palette,
}

/// The ground height at the spawn (blocks 0..=64 are solid there).
pub const SPAWN_GROUND: i32 = 64;
/// Radius of the flat area around the spawn column (16, 16).
const FLAT: f64 = 12.0;

fn hash(seed: u64, x: i64, z: i64) -> u64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h
}

fn unit(seed: u64, x: i64, z: i64) -> f64 {
    (hash(seed, x, z) >> 11) as f64 / (1u64 << 53) as f64
}

/// Smooth value noise in 0..1 with cells of `cell` blocks.
fn value(seed: u64, x: f64, z: f64, cell: f64) -> f64 {
    let (fx, fz) = (x / cell, z / cell);
    let (x0, z0) = (fx.floor(), fz.floor());
    let (tx, tz) = (fx - x0, fz - z0);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (s(tx), s(tz));
    let v = |dx: i64, dz: i64| unit(seed, x0 as i64 + dx, z0 as i64 + dz);
    let a = v(0, 0) + (v(1, 0) - v(0, 0)) * sx;
    let b = v(0, 1) + (v(1, 1) - v(0, 1)) * sx;
    a + (b - a) * sz
}

impl Generator {
    /// Height of the grass block in column (x, z).
    pub fn height(&self, x: i32, z: i32) -> i32 {
        let (fx, fz) = (x as f64, z as f64);
        let hills =
            value(self.seed, fx, fz, 48.0) * 18.0 + value(self.seed ^ 1, fx, fz, 16.0) * 5.0;
        let h = 58.0 + hills;
        // Flat near the spawn, blending out to the hills.
        let d = ((fx - 16.0).powi(2) + (fz - 16.0).powi(2)).sqrt();
        let t = ((d - FLAT) / FLAT).clamp(0.0, 1.0);
        (SPAWN_GROUND as f64 * (1.0 - t) + h * t).round() as i32
    }

    /// A tree stands on (x, z): about one column in 90, not near the spawn.
    fn tree_at(&self, x: i32, z: i32) -> bool {
        let d2 = (x - 16).pow(2) + (z - 16).pow(2);
        d2 > 100 && hash(self.seed ^ 7, x as i64, z as i64) % 90 == 0
    }

    /// The block at (x, y, z) of the world.
    pub fn block(&self, x: i32, y: i32, z: i32) -> i32 {
        let p = &self.palette;
        let h = self.height(x, z);
        if y == 0 {
            return p.bedrock;
        }
        if y <= h {
            return if y == h {
                p.grass
            } else if y >= h - 3 {
                p.dirt
            } else {
                p.stone
            };
        }
        // Trees: a trunk of 5 and a round crown of leaves around its top.
        for tx in x - 2..=x + 2 {
            for tz in z - 2..=z + 2 {
                if !self.tree_at(tx, tz) {
                    continue;
                }
                let base = self.height(tx, tz);
                let top = base + 5;
                if tx == x && tz == z && y > base && y <= top {
                    return p.trunk;
                }
                let (dx, dy, dz) = (x - tx, y - top, z - tz);
                if (-1..=2).contains(&dy)
                    && dx * dx + dy * dy + dz * dz <= 5
                    && !(dx == 0 && dz == 0 && dy <= 0)
                {
                    return p.leaves;
                }
            }
        }
        0
    }

    /// Section (cx, cy, cz).
    pub fn section(&self, cx: i32, cy: i32, cz: i32) -> Section {
        let mut s = Section::default();
        let (bx, by, bz) = (cx * SIZE as i32, cy * SIZE as i32, cz * SIZE as i32);
        for z in 0..SIZE {
            for x in 0..SIZE {
                let (wx, wz) = (bx + x as i32, bz + z as i32);
                // Nothing above the highest leaf of any tree nearby.
                let ceiling = self.height(wx, wz) + 8;
                for y in 0..SIZE {
                    let wy = by + y as i32;
                    if wy > ceiling {
                        break;
                    }
                    let b = self.block(wx, wy, wz);
                    if b != 0 {
                        s.blocks[index(x, y, z)] = b;
                    }
                }
            }
        }
        s
    }

    /// `SetColumn.heightmap` of column (cx, cz): the grass height, index
    /// z * 32 + x.
    pub fn heightmap(&self, cx: i32, cz: i32) -> Vec<i32> {
        let mut v = Vec::with_capacity(SIZE * SIZE);
        for z in 0..SIZE as i32 {
            for x in 0..SIZE as i32 {
                v.push(self.height(cx * SIZE as i32 + x, cz * SIZE as i32 + z));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generator() -> Generator {
        Generator {
            seed: 42,
            palette: Palette {
                bedrock: 1,
                stone: 2,
                dirt: 3,
                grass: 4,
                trunk: 5,
                leaves: 6,
            },
        }
    }

    #[test]
    fn spawn_is_flat_and_on_grass() {
        let g = generator();
        assert_eq!(g.height(16, 16), SPAWN_GROUND);
        assert_eq!(g.block(16, SPAWN_GROUND, 16), 4);
        assert_eq!(g.block(16, SPAWN_GROUND + 1, 16), 0);
        assert_eq!(g.block(16, 0, 16), 1);
    }

    #[test]
    fn hills_and_trees_exist() {
        let g = generator();
        let hs: Vec<i32> = (0..200).map(|i| g.height(200 + i, -150)).collect();
        assert!(
            hs.iter().max().unwrap() - hs.iter().min().unwrap() > 4,
            "{hs:?}"
        );
        let trees = (-100..100)
            .flat_map(|x| (-100..100).map(move |z| (x, z)))
            .filter(|&(x, z)| g.tree_at(x, z))
            .count();
        assert!(trees > 100, "{trees}");
        // Sections agree with blocks.
        let s = g.section(3, 2, -2);
        assert_eq!(s.blocks[index(5, 1, 7)], g.block(96 + 5, 65, -64 + 7));
    }
}
