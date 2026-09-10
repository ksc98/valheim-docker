use crate::files::FileManager;
use chrono::Utc;
use log::{debug, error};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Save durations kept for the histogram (a save every 10 min ≈ 3.5 days).
const MAX_SAVES: usize = 500;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Save {
  /// `save` (world autosave) or `backup` (world auto backup)
  pub kind: String,
  pub seconds: f64,
  /// Unix time the save finished
  pub at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct WrongPassword {
  pub steam_id: String,
  /// Steam display name from the server's player history, if the id has joined before
  pub name: String,
  pub count: u64,
}

/// World-level facts parsed from `valheim_server.log`, kept in `world.stats` for Huginn.
/// Survives restarts; `odin start` only resets the per-boot fields.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct WorldStats {
  pub zdo_count: Option<u64>,
  pub connections: Option<u32>,
  pub day: Option<u32>,
  pub load_seconds: Option<f64>,
  pub rpc_timeouts: u64,
  pub wrong_password: Vec<WrongPassword>,
  pub saves: Vec<Save>,
  /// Unix time `odin start` launched the server, for `load_seconds`
  #[serde(default)]
  boot_started: Option<i64>,
  #[serde(default)]
  steam_names: BTreeMap<String, String>,
}

impl FileManager for WorldStats {
  fn path(&self) -> String {
    format!(
      "{}/world.stats",
      crate::utils::common_paths::saves_directory()
    )
  }
}

impl WorldStats {
  pub fn load() -> Self {
    let content = WorldStats::default().read();
    if content.trim().is_empty() {
      return WorldStats::default();
    }
    serde_json::from_str(&content).unwrap_or_else(|e| {
      error!("Failed to parse world.stats, starting fresh: {e}");
      WorldStats::default()
    })
  }

  fn save(&self) {
    match serde_json::to_string_pretty(self) {
      Ok(content) => {
        self.write(content);
      }
      Err(e) => error!("Failed to serialize world.stats: {e}"),
    }
  }

  /// Marks a server start: records when it launched and drops the per-boot fields.
  pub fn server_started() {
    let mut stats = WorldStats::load();
    stats.boot_started = Some(Utc::now().timestamp());
    stats.load_seconds = None;
    stats.connections = None;
    stats.save();
  }

  fn record_save(&mut self, kind: &str, ms: &str) {
    let Ok(ms) = ms.replace(',', "").parse::<f64>() else {
      return;
    };
    self.saves.push(Save {
      kind: kind.to_string(),
      seconds: ms / 1000.0,
      at: Utc::now().timestamp(),
    });
    if self.saves.len() > MAX_SAVES {
      self.saves.drain(..self.saves.len() - MAX_SAVES);
    }
  }

  fn record_wrong_password(&mut self, steam_id: &str) {
    let name = self.steam_names.get(steam_id).cloned().unwrap_or_default();
    match self
      .wrong_password
      .iter_mut()
      .find(|w| w.steam_id == steam_id)
    {
      Some(w) => w.count += 1,
      None => self.wrong_password.push(WrongPassword {
        steam_id: steam_id.to_string(),
        name,
        count: 1,
      }),
    }
  }

  /// Applies one log line; returns true if anything changed.
  fn apply(&mut self, line: &str) -> bool {
    if let Some(c) = CONNECTIONS.captures(line) {
      self.connections = c[1].parse().ok();
      self.zdo_count = c[2].parse().ok();
    } else if let Some(c) = LOAD_CHUNKS.captures(line) {
      self.zdo_count = c[1].replace(',', "").parse().ok();
    } else if let Some(c) = DAY.captures(line) {
      self.day = c[1].parse().ok();
    } else if let Some(c) = WORLD_SAVE.captures(line) {
      self.record_save("save", &c[1]);
    } else if let Some(c) = BACKUP.captures(line) {
      self.record_save("backup", &c[1]);
    } else if CONNECTED.is_match(line) {
      if let Some(start) = self.boot_started {
        self.load_seconds = Some((Utc::now().timestamp() - start) as f64);
      }
    } else if RPC_TIMEOUT.is_match(line) {
      self.rpc_timeouts += 1;
    } else if let Some(c) = WRONG_PASSWORD.captures(line) {
      self.record_wrong_password(&c[1]);
    } else if let Some(c) = HISTORY.captures(line) {
      self.steam_names.insert(c[2].to_string(), c[1].to_string());
    } else {
      return false;
    }
    true
  }
}

static CONNECTIONS: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Connections (\d+) ZDOS:(\d+)").unwrap());
static LOAD_CHUNKS: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"ZDOMan\.LoadChunks - Starting to load ([\d,]+) zdos").unwrap());
static DAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Time [\d.]+, day:(\d+)\s").unwrap());
static WORLD_SAVE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"World save \(\d+/\d+\) done\. Total time \[([\d,.]+)ms\]").unwrap()
});
static BACKUP: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"World auto backup saved \[([\d,.]+)ms\]").unwrap());
static CONNECTED: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Game server connected$").unwrap());
static RPC_TIMEOUT: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"ZRpc timeout detected").unwrap());
static WRONG_PASSWORD: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Peer (\d+) has wrong password").unwrap());
static HISTORY: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"Player history entry with index \d+:\s+(.+?) \(Steam_(\d+),").unwrap()
});

/// Updates `world.stats` from a server log line.
pub fn handle_world_events(line: &str) {
  // cheap pre-check so the file is only touched for lines we track
  if !line.contains("ZDOS:")
    && !line.contains("Starting to load")
    && !line.contains("skipspeed:")
    && !line.contains("World save (")
    && !line.contains("auto backup saved")
    && !line.contains("Game server connected")
    && !line.contains("ZRpc timeout detected")
    && !line.contains("has wrong password")
    && !line.contains("Player history entry")
  {
    return;
  }
  let mut stats = WorldStats::load();
  if stats.apply(line) {
    debug!("world.stats updated from: {line}");
    stats.save();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_world_lines() {
    let mut s = WorldStats::default();
    assert!(s.apply("09/09/2026 21:45:56:  Connections 3 ZDOS:76806  sent:250 recv:1009"));
    assert_eq!(s.zdo_count, Some(76806));
    assert_eq!(s.connections, Some(3));
    assert!(s.apply(
      "09/09/2026 23:39:40: ZDOMan.LoadChunks - Starting to load 77,904 zdos from 12 Chunks. SessionID: 1, WorldVersion: 35 [DeepNorth]"
    ));
    assert_eq!(s.zdo_count, Some(77904));
    assert!(s.apply(
      "09/09/2026 21:31:21: Time 8893.43944863975, day:4    nextm:9270.00001072884  skipspeed:31.38"
    ));
    assert_eq!(s.day, Some(4));
    assert!(s.apply("09/09/2026 23:49:31: World save (5/5) done. Total time [2,844ms]"));
    assert!(s.apply("09/09/2026 19:44:05: World auto backup saved [2975ms]"));
    assert_eq!(s.saves.len(), 2);
    assert_eq!(s.saves[0].kind, "save");
    assert!((s.saves[0].seconds - 2.844).abs() < 1e-9);
    assert_eq!(s.saves[1].kind, "backup");
    s.boot_started = Some(Utc::now().timestamp() - 52);
    assert!(s.apply("09/09/2026 23:40:04: Game server connected"));
    assert!(s.load_seconds.is_some_and(|l| (52.0..54.0).contains(&l)));
    assert!(s.apply("09/09/2026 22:00:00: ZRpc timeout detected"));
    assert_eq!(s.rpc_timeouts, 1);
    assert!(s.apply("09/09/2026 22:41:31: Player history entry with index 2:  Viking (Steam_76561190000000000, 164E6C13B3370AED)"));
    assert!(s.apply("09/09/2026 21:44:13: Peer 76561190000000000 has wrong password"));
    assert!(s.apply("09/09/2026 21:44:20: Peer 76561190000000000 has wrong password"));
    assert!(s.apply("09/09/2026 21:44:30: Peer 76561190000000001 has wrong password"));
    assert_eq!(s.wrong_password.len(), 2);
    assert_eq!(s.wrong_password[0].count, 2);
    assert_eq!(s.wrong_password[0].name, "Viking");
    assert_eq!(s.wrong_password[1].name, "");
    assert!(!s.apply("09/09/2026 22:00:00: Sending message to save player profiles"));
  }

  #[test]
  fn keeps_bounded_save_history() {
    let mut s = WorldStats::default();
    for _ in 0..(MAX_SAVES + 10) {
      s.record_save("save", "1000");
    }
    assert_eq!(s.saves.len(), MAX_SAVES);
  }
}
