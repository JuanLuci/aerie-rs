//! Reading values out of asset JSON (spec 07, "Values").

use aerie_protocol::proto::{Color, ColorAlpha};
use serde_json::Value;

/// Typed accessors with the defaults Hytale assets use.
pub trait Get {
    fn str_at(&self, key: &str) -> Option<String>;
    fn bool_or(&self, key: &str, default: bool) -> bool;
    fn i32_or(&self, key: &str, default: i32) -> i32;
    fn f32_or(&self, key: &str, default: f32) -> f32;
    fn f64_or(&self, key: &str, default: f64) -> f64;
    fn opt_f32(&self, key: &str) -> Option<f32>;
    fn opt_i32(&self, key: &str) -> Option<i32>;
    fn strings(&self, key: &str) -> Option<Vec<String>>;
    fn color(&self, key: &str) -> Option<Color>;
    fn color_alpha(&self, key: &str) -> Option<ColorAlpha>;
    fn obj(&self, key: &str) -> Option<&Value>;
}

impl Get for Value {
    fn str_at(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Value::as_str).map(str::to_owned)
    }
    fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }
    fn i32_or(&self, key: &str, default: i32) -> i32 {
        self.opt_i32(key).unwrap_or(default)
    }
    fn f32_or(&self, key: &str, default: f32) -> f32 {
        self.opt_f32(key).unwrap_or(default)
    }
    fn f64_or(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(Value::as_f64).unwrap_or(default)
    }
    fn opt_f32(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(Value::as_f64).map(|v| v as f32)
    }
    fn opt_i32(&self, key: &str) -> Option<i32> {
        self.get(key)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .map(|v| v as i32)
    }
    fn strings(&self, key: &str) -> Option<Vec<String>> {
        self.get(key)?.as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
    }
    fn color(&self, key: &str) -> Option<Color> {
        self.get(key).and_then(Value::as_str).and_then(parse_color)
    }
    fn color_alpha(&self, key: &str) -> Option<ColorAlpha> {
        self.get(key)
            .and_then(Value::as_str)
            .and_then(parse_color_alpha)
    }
    fn obj(&self, key: &str) -> Option<&Value> {
        self.get(key).filter(|v| v.is_object())
    }
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` → RGBA bytes (alpha 255 when absent).
pub fn hex_rgba(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    let nib = |x: u32| ((x & 15) * 17) as u8;
    Some(match h.len() {
        3 => [nib(v >> 8), nib(v >> 4), nib(v), 255],
        4 => [nib(v >> 12), nib(v >> 8), nib(v >> 4), nib(v)],
        6 => [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255],
        8 => [(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8],
        _ => return None,
    })
}

fn channels(s: &str, prefix: &str) -> Option<Vec<String>> {
    let s = s.trim();
    let inner = s.strip_prefix(prefix)?.strip_suffix(')')?;
    Some(inner.split(',').map(|c| c.trim().to_owned()).collect())
}

/// Alpha as the server computes it: clamp(a × 255, 0, 255), truncated.
fn alpha_byte(a: &str) -> Option<u8> {
    let f: f32 = a.trim().parse().ok()?;
    Some((f * 255.0).clamp(0.0, 255.0) as u8)
}

pub fn parse_color(s: &str) -> Option<Color> {
    let (r, g, b) = if s.trim_start().starts_with('#') {
        let [r, g, b, _] = hex_rgba(s)?;
        (r, g, b)
    } else {
        let c = channels(s, "rgb(")?;
        if c.len() != 3 {
            return None;
        }
        (
            c[0].parse::<i32>().ok()? as u8,
            c[1].parse::<i32>().ok()? as u8,
            c[2].parse::<i32>().ok()? as u8,
        )
    };
    Some(Color {
        red: r as i8,
        green: g as i8,
        blue: b as i8,
    })
}

pub fn parse_color_alpha(s: &str) -> Option<ColorAlpha> {
    let t = s.trim();
    let [r, g, b, a] = if t.starts_with('#') {
        hex_rgba(t)?
    } else {
        let c = channels(t, "rgba(")?;
        match c.len() {
            2 => {
                let [r, g, b, _] = hex_rgba(&c[0])?;
                [r, g, b, alpha_byte(&c[1])?]
            }
            4 => {
                let ch = |x: &str| x.parse::<i32>().ok().map(|v| v.clamp(0, 255) as u8);
                [ch(&c[0])?, ch(&c[1])?, ch(&c[2])?, alpha_byte(&c[3])?]
            }
            _ => return None,
        }
    };
    Some(ColorAlpha {
        alpha: a as i8,
        red: r as i8,
        green: g as i8,
        blue: b as i8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        let c = parse_color("#c9d2dd").unwrap();
        assert_eq!((c.red, c.green, c.blue), (-55, -46, -35));
        let c = parse_color("#a72").unwrap();
        assert_eq!(
            (c.red as u8, c.green as u8, c.blue as u8),
            (0xaa, 0x77, 0x22)
        );
        let a = parse_color_alpha("rgba(#ff0000, 0.5)").unwrap();
        assert_eq!((a.red as u8, a.alpha as u8), (255, 127));
        let a = parse_color_alpha("rgba(10, 20, 30, 1)").unwrap();
        assert_eq!((a.blue, a.alpha as u8), (30, 255));
    }
}

/// `{"X", "Y", "Z"}`, missing components 0.
pub fn vec3(v: &Value) -> [f32; 3] {
    [v.f32_or("X", 0.0), v.f32_or("Y", 0.0), v.f32_or("Z", 0.0)]
}

/// `{"Yaw", "Pitch", "Roll"}`, missing components 0.
pub fn direction(v: &Value) -> aerie_protocol::proto::Direction {
    aerie_protocol::proto::Direction {
        yaw: v.f32_or("Yaw", 0.0),
        pitch: v.f32_or("Pitch", 0.0),
        roll: v.f32_or("Roll", 0.0),
    }
}
