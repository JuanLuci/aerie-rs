//! How heavy the server runs (spec 13): tick times, ticks per second,
//! memory, loaded columns, entities and players. A small panel in a
//! corner of the screen for players who turn it on (a custom HUD made of
//! inline UI markup, as the official server's own HUDs), and a `perf:` log
//! line every 10 s.

use std::time::{Duration, Instant};

use aerie_protocol::proto;

/// The custom HUD's key on the client.
pub const HUD_ID: &str = "AeriePerf";
/// The world tick's budget (30 Hz).
pub const TARGET_MS: f64 = 1000.0 / 30.0;

/// Where the panel sits and what it holds: a dark box in the top right
/// corner with one small line per value.
const DOCUMENT: &str = "Group #AeriePerf {
  Anchor: (Right: 12, Top: 12, Width: 190);
  LayoutMode: Top;
  Background: #000000(0.45);

  Label #PerfTick {
    Anchor: (Height: 16, Horizontal: 6, Top: 4);
    Style: (FontSize: 12, TextColor: #c8d6d8);
    Text: \"\";
  }
  Label #PerfTps {
    Anchor: (Height: 16, Horizontal: 6);
    Style: (FontSize: 12, TextColor: #c8d6d8);
    Text: \"\";
  }
  Label #PerfRam {
    Anchor: (Height: 16, Horizontal: 6);
    Style: (FontSize: 12, TextColor: #c8d6d8);
    Text: \"\";
  }
  Label #PerfWorld {
    Anchor: (Height: 16, Horizontal: 6, Bottom: 4);
    Style: (FontSize: 12, TextColor: #c8d6d8);
    Text: \"\";
  }
}
";

/// Tick times over a stretch of time.
#[derive(Clone, Debug, Default)]
struct Window {
    ticks: u32,
    sum_ms: f64,
    max_ms: f64,
    peak_ram: u64,
    peak_resident: u64,
}

impl Window {
    fn add(&mut self, ms: f64) {
        self.ticks += 1;
        self.sum_ms += ms;
        self.max_ms = self.max_ms.max(ms);
    }

    fn avg_ms(&self) -> f64 {
        if self.ticks == 0 {
            0.0
        } else {
            self.sum_ms / self.ticks as f64
        }
    }
}

/// What the world holds, counted by the hub.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub columns: usize,
    pub entities: usize,
    pub players: usize,
}

/// One second's (or ten seconds') numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The process's own memory: private bytes on Windows (its resident
    /// set shrinks when other programs need memory), the resident set
    /// elsewhere.
    pub ram: u64,
    pub resident: u64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub tps: f64,
    pub counts: Counts,
}

impl Sample {
    /// The `perf:` log line.
    pub fn log_line(&self) -> String {
        format!(
            "perf: ram {:.1} MB, tick avg {:.2} ms max {:.2} ms (budget {:.1}), tps {:.1}, \
             columns {}, entities {}, players {}, resident {:.1} MB",
            mb(self.ram),
            self.avg_ms,
            self.max_ms,
            TARGET_MS,
            self.tps,
            self.counts.columns,
            self.counts.entities,
            self.counts.players,
            mb(self.resident)
        )
    }

    /// The panel's lines (tick, ticks per second, memory, world).
    pub fn lines(&self) -> [(&'static str, String); 4] {
        [
            (
                "#PerfTick.Text",
                format!("tick {:.2} / {:.2} ms", self.avg_ms, self.max_ms),
            ),
            ("#PerfTps.Text", format!("TPS {:.1}", self.tps)),
            (
                "#PerfRam.Text",
                format!("RAM {:.0} MB ({:.0} res)", mb(self.ram), mb(self.resident)),
            ),
            (
                "#PerfWorld.Text",
                format!(
                    "chunks {}  ent {}  pl {}",
                    self.counts.columns, self.counts.entities, self.counts.players
                ),
            ),
        ]
    }
}

fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

/// The process's memory in bytes (0 if the system does not say): its own
/// (private on Windows, resident elsewhere) and resident.
pub fn ram() -> (u64, u64) {
    let Some(m) = memory_stats::memory_stats() else {
        return (0, 0);
    };
    let resident = m.physical_mem as u64;
    let own = if cfg!(windows) {
        m.virtual_mem as u64
    } else {
        resident
    };
    (own, resident)
}

/// Tick times as the tick task reports them.
#[derive(Debug)]
pub struct Perf {
    second: Window,
    ten: Window,
    second_from: Instant,
    ten_from: Instant,
}

impl Default for Perf {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            second: Window::default(),
            ten: Window::default(),
            second_from: now,
            ten_from: now,
        }
    }
}

/// What a tick report gives back: a new panel sample each second, a log
/// line each ten.
#[derive(Debug, Default)]
pub struct Due {
    pub second: Option<Sample>,
    pub ten: Option<Sample>,
}

impl Perf {
    /// One tick took `took`; `counts` is asked for only when a sample is
    /// due.
    pub fn tick(&mut self, took: Duration, now: Instant, counts: impl Fn() -> Counts) -> Due {
        let ms = took.as_secs_f64() * 1000.0;
        self.second.add(ms);
        self.ten.add(ms);
        let mut due = Due::default();
        let s = now.duration_since(self.second_from);
        if s >= Duration::from_secs(1) {
            let (r, res) = ram();
            self.ten.peak_ram = self.ten.peak_ram.max(r);
            self.ten.peak_resident = self.ten.peak_resident.max(res);
            let c = counts();
            due.second = Some(Sample {
                ram: r,
                resident: res,
                avg_ms: self.second.avg_ms(),
                max_ms: self.second.max_ms,
                tps: self.second.ticks as f64 / s.as_secs_f64(),
                counts: c,
            });
            self.second = Window::default();
            self.second_from = now;
            let t = now.duration_since(self.ten_from);
            if t >= Duration::from_secs(10) {
                due.ten = Some(Sample {
                    ram: self.ten.peak_ram,
                    resident: self.ten.peak_resident,
                    avg_ms: self.ten.avg_ms(),
                    max_ms: self.ten.max_ms,
                    tps: self.ten.ticks as f64 / t.as_secs_f64(),
                    counts: c,
                });
                self.ten = Window::default();
                self.ten_from = now;
            }
        }
        due
    }
}

fn set(selector: &str, text: &str) -> proto::CustomUICommand {
    proto::CustomUICommand {
        r#type: proto::CustomUICommandType::SET,
        selector: Some(selector.to_owned()),
        data: Some(serde_json::json!({ "0": text }).to_string()),
        text: None,
    }
}

/// The panel's first showing: its markup, then the values.
pub fn show(s: &Sample) -> proto::CustomHud {
    let mut commands = vec![proto::CustomUICommand {
        r#type: proto::CustomUICommandType::APPEND_INLINE,
        selector: None,
        data: None,
        text: Some(DOCUMENT.to_owned()),
    }];
    commands.extend(s.lines().iter().map(|(k, v)| set(k, v)));
    proto::CustomHud {
        hud_id: HUD_ID.to_owned(),
        z_order: 0,
        clear: true,
        commands: Some(commands),
    }
}

/// New values for a shown panel.
pub fn update(s: &Sample) -> proto::CustomHud {
    proto::CustomHud {
        hud_id: HUD_ID.to_owned(),
        z_order: 0,
        clear: false,
        commands: Some(s.lines().iter().map(|(k, v)| set(k, v)).collect()),
    }
}

/// Removes the panel (as the official server removes a custom HUD).
pub fn hide() -> proto::CustomHud {
    proto::CustomHud {
        hud_id: HUD_ID.to_owned(),
        z_order: 0,
        clear: true,
        commands: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_each_second_and_ten() {
        let t0 = Instant::now();
        let mut p = Perf {
            second: Window::default(),
            ten: Window::default(),
            second_from: t0,
            ten_from: t0,
        };
        let c = || Counts {
            columns: 225,
            entities: 3,
            players: 1,
        };
        let mut seconds = 0;
        let mut tens = 0;
        for i in 1..=300u64 {
            let now = t0 + Duration::from_millis(i * 1000 / 30);
            let took = Duration::from_millis(if i == 45 { 40 } else { 2 });
            let d = p.tick(took, now, c);
            if let Some(s) = d.second {
                seconds += 1;
                assert!((s.tps - 30.0).abs() < 1.5, "{s:?}");
            }
            if let Some(s) = d.ten {
                tens += 1;
                assert_eq!(s.max_ms, 40.0);
                assert!(s.log_line().starts_with("perf: ram "));
            }
        }
        assert_eq!((seconds, tens), (10, 1));
    }

    #[test]
    fn panel_commands() {
        let s = Sample {
            ram: 64 << 20,
            resident: 10 << 20,
            avg_ms: 1.25,
            max_ms: 3.5,
            tps: 30.0,
            counts: Counts::default(),
        };
        let h = show(&s);
        let c = h.commands.unwrap();
        assert_eq!(c[0].r#type, proto::CustomUICommandType::APPEND_INLINE);
        assert_eq!(c[3].data.as_deref(), Some(r#"{"0":"RAM 64 MB (10 res)"}"#));
        assert!(hide().commands.is_none());
    }
}
