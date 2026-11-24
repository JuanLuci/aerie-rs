//! The server's configuration file (spec 25).
//!
//! `config.json` keeps the layout and key names of the official server's
//! file, so a file from that server can be reused. Only the settings Aerie
//! has are read and written; every other key is kept exactly as it is.
//! Command-line options override the file for the run (nothing the options
//! say is written back).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use tracing::{info, warn};

/// The file's own version, written when Aerie creates the file.
const VERSION: i64 = 4;
/// The view radius is kept between these (in chunks).
const VIEW_RADIUS_RANGE: (i64, i64) = (1, 32);

/// What the settings are when the file or a key is missing.
pub const DEFAULT_NAME: &str = "Aerie Server";
const DEFAULT_MAX_PLAYERS: i64 = 100;
const DEFAULT_GAME_MODE: &str = "Adventure";

/// The settings Aerie uses, as read from the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub name: String,
    pub motd: String,
    /// Empty means no password.
    pub password: String,
    /// Zero or less means no limit.
    pub max_players: i32,
    /// In chunks, between 1 and 32.
    pub max_view_radius: i32,
    pub creative: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.into(),
            motd: String::new(),
            password: String::new(),
            max_players: DEFAULT_MAX_PLAYERS as i32,
            max_view_radius: 32,
            creative: false,
        }
    }
}

/// The keys Aerie writes into a file that lacks them.
fn defaults_json() -> Map<String, Value> {
    let d = Config::default();
    let Value::Object(m) = json!({
        "Version": VERSION,
        "ServerName": d.name,
        "MOTD": d.motd,
        "Password": d.password,
        "MaxPlayers": d.max_players,
        "MaxViewRadius": d.max_view_radius,
        "Defaults": { "GameMode": DEFAULT_GAME_MODE },
    }) else {
        unreachable!()
    };
    m
}

/// Adds what is missing, leaving every present key (known or not) alone.
/// Returns whether anything was added.
fn fill(file: &mut Map<String, Value>) -> bool {
    let mut added = false;
    for (k, v) in defaults_json() {
        match file.get_mut(&k) {
            None => {
                file.insert(k, v);
                added = true;
            }
            // `Defaults` holds more keys the file may already have.
            Some(Value::Object(have)) => {
                if let Value::Object(want) = v {
                    for (k, v) in want {
                        if !have.contains_key(&k) {
                            have.insert(k, v);
                            added = true;
                        }
                    }
                }
            }
            Some(_) => {}
        }
    }
    added
}

fn text(file: &Map<String, Value>, key: &str, default: &str) -> String {
    match file.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => {
            warn!("config: {key} should be text, not {other}; using {default:?}");
            default.into()
        }
        None => default.into(),
    }
}

fn number(file: &Map<String, Value>, key: &str, default: i64) -> i64 {
    match file.get(key) {
        Some(v) => v.as_i64().unwrap_or_else(|| {
            warn!("config: {key} should be a whole number, not {v}; using {default}");
            default
        }),
        None => default,
    }
}

impl Config {
    /// Reads the settings out of a parsed file.
    pub fn from_json(file: &Map<String, Value>) -> Self {
        let max_view = number(file, "MaxViewRadius", 32);
        let (lo, hi) = VIEW_RADIUS_RANGE;
        let clamped = max_view.clamp(lo, hi);
        if clamped != max_view {
            warn!("config: MaxViewRadius {max_view} is outside [{lo}, {hi}], using {clamped}");
        }
        let mode = match file.get("Defaults") {
            Some(Value::Object(d)) => text(d, "GameMode", DEFAULT_GAME_MODE),
            _ => DEFAULT_GAME_MODE.into(),
        };
        let creative = match mode.to_ascii_lowercase().as_str() {
            "creative" => true,
            "adventure" => false,
            _ => {
                warn!(
                    "config: Defaults.GameMode {mode:?} is not Adventure or Creative; using Adventure"
                );
                false
            }
        };
        Self {
            name: text(file, "ServerName", DEFAULT_NAME),
            motd: text(file, "MOTD", ""),
            password: text(file, "Password", ""),
            max_players: number(file, "MaxPlayers", DEFAULT_MAX_PLAYERS)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            max_view_radius: clamped as i32,
            creative,
        }
    }

    /// Reads the file, creating it with the defaults when it is missing and
    /// adding the keys it lacks. A file that is not a JSON object stops the
    /// start: it is never overwritten.
    pub fn load(path: &Path) -> Result<Self> {
        let mut file = match std::fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<Value>(&s)
                .with_context(|| format!("{} is not valid JSON", path.display()))?
            {
                Value::Object(m) => m,
                _ => bail!("{} must hold a JSON object", path.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("{} not found: writing the defaults", path.display());
                Map::new()
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let was_empty = file.is_empty();
        if fill(&mut file) {
            if !was_empty {
                info!("{}: adding the keys it lacked", path.display());
            }
            save(path, &file)?;
        }
        Ok(Self::from_json(&file))
    }
}

/// Writes the file whole through a temporary one, so that a crash never
/// leaves half of it.
fn save(path: &Path, file: &Map<String, Value>) -> Result<()> {
    let mut out = serde_json::to_string_pretty(file)?;
    out.push('\n');
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// What the command line says, applied over the file for this run.
#[derive(Debug, Default)]
pub struct Overrides {
    pub name: Option<String>,
    pub motd: Option<String>,
    pub password: Option<String>,
    pub max_players: Option<i32>,
    pub max_view_radius: Option<i32>,
    pub game_mode: Option<String>,
}

impl Config {
    pub fn with_overrides(mut self, o: Overrides) -> Result<Self> {
        if let Some(v) = o.name {
            self.name = v;
        }
        if let Some(v) = o.motd {
            self.motd = v;
        }
        if let Some(v) = o.password {
            self.password = v;
        }
        if let Some(v) = o.max_players {
            self.max_players = v;
        }
        if let Some(v) = o.max_view_radius {
            let (lo, hi) = VIEW_RADIUS_RANGE;
            self.max_view_radius = v.clamp(lo as i32, hi as i32);
        }
        if let Some(m) = o.game_mode {
            // The option has always meant "creative or not".
            self.creative = m.eq_ignore_ascii_case("creative");
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(s: &str) -> Map<String, Value> {
        match serde_json::from_str(s).unwrap() {
            Value::Object(m) => m,
            _ => panic!(),
        }
    }

    #[test]
    fn missing_file_gets_the_defaults() {
        let dir = std::env::temp_dir().join(format!("aerie-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let _ = std::fs::remove_file(&path);
        let c = Config::load(&path).unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.name, "Aerie Server");
        let written = obj(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(Config::from_json(&written), c);
        // A second start reads it back and changes nothing.
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_keys_and_their_order_survive() {
        let dir = std::env::temp_dir().join(format!("aerie-config-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        // A file with a key Aerie does not know, and one setting missing.
        std::fs::write(
            &path,
            r#"{"Zed": {"a": [1, 2.5, null]}, "ServerName": "X", "Mods": {"k": true}, "MaxPlayers": 3}"#,
        )
        .unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!((c.name.as_str(), c.max_players), ("X", 3));
        let now = obj(&std::fs::read_to_string(&path).unwrap());
        let keys: Vec<_> = now.keys().cloned().collect();
        assert_eq!(&keys[..3], ["Zed", "ServerName", "Mods"]);
        assert_eq!(now["Zed"], json!({"a": [1, 2.5, null]}));
        assert_eq!(now["Mods"], json!({"k": true}));
        assert!(now.contains_key("MaxViewRadius"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_values_fall_back_and_the_radius_is_clamped() {
        let c = Config::from_json(&obj(
            r#"{"ServerName": 5, "MaxPlayers": "many", "MaxViewRadius": 99,
                "Defaults": {"GameMode": "creative"}}"#,
        ));
        assert_eq!(c.name, DEFAULT_NAME);
        assert_eq!(c.max_players, 100);
        assert_eq!(c.max_view_radius, 32);
        assert!(c.creative);
        let c = Config::from_json(&obj(r#"{"MaxViewRadius": 0}"#));
        assert_eq!(c.max_view_radius, 1);
    }

    #[test]
    fn options_win_over_the_file() {
        let c = Config::default()
            .with_overrides(Overrides {
                name: Some("N".into()),
                max_players: Some(1),
                game_mode: Some("Creative".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!((c.name.as_str(), c.max_players, c.creative), ("N", 1, true));
        assert_eq!(c.max_view_radius, 32);
    }
}
