//! The official server's fast sine and cosine: a table of 4096 values
//! (Riven's well-known lookup method), so directions match it to the bit.

use std::sync::OnceLock;

const COUNT: usize = 4096;
const MASK: i32 = COUNT as i32 - 1;

struct Tables {
    sin: Vec<f32>,
    cos: Vec<f32>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let full = std::f64::consts::TAU as f32;
        let mut sin = vec![0f32; COUNT];
        let mut cos = vec![0f32; COUNT];
        for i in 0..COUNT {
            let a = (i as f32 + 0.5) / COUNT as f32 * full;
            sin[i] = (a as f64).sin() as f32;
            cos[i] = (a as f64).cos() as f32;
        }
        let deg = COUNT as f32 / 360.0;
        for d in (0..360).step_by(90) {
            let i = ((d as f32 * deg) as i32 & MASK) as usize;
            let r = d as f64 * std::f64::consts::PI / 180.0;
            sin[i] = r.sin() as f32;
            cos[i] = r.cos() as f32;
        }
        Tables { sin, cos }
    })
}

fn index(rad: f32) -> usize {
    let to_index = COUNT as f32 / std::f64::consts::TAU as f32;
    ((rad * to_index) as i32 & MASK) as usize
}

pub fn sin(rad: f32) -> f32 {
    tables().sin[index(rad)]
}

pub fn cos(rad: f32) -> f32 {
    tables().cos[index(rad)]
}

/// The direction a pitch and yaw look along (the official formula).
pub fn direction(pitch: f32, yaw: f32) -> [f64; 3] {
    let len = cos(pitch) as f64;
    [
        len * -(sin(yaw) as f64),
        sin(pitch) as f64,
        len * -(cos(yaw) as f64),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn quarter_turns_are_exact() {
        assert_eq!(super::sin(0.0), 0.0);
        assert_eq!(super::cos(0.0), 1.0);
        let d = super::direction(0.0, 0.0);
        assert_eq!(d, [-0.0, 0.0, -1.0]);
    }
}
