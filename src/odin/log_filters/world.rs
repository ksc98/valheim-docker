use crate::files::FileManager;
use crate::utils::common_paths::saves_directory;
use crate::utils::environment::fetch_var;
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

/// One Unity garbage-collection pause.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GcPause {
  pub seconds: f64,
  /// Unix time the pause was logged
  pub at: i64,
}

/// GC pauses kept for the histogram.
const MAX_GC_PAUSES: usize = 500;

/// Result of the last `odin update --check`.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct UpdateCheck {
  pub current_build: String,
  pub latest_build: String,
  pub available: bool,
  /// Unix time of the check
  pub checked_at: i64,
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
  /// Size of the world on disk after the last save (and at boot): the chunked
  /// `worlds_local/<world>/` directory, or the legacy `<world>.db` file
  #[serde(default)]
  pub world_bytes: Option<u64>,
  #[serde(default)]
  pub update: Option<UpdateCheck>,
  /// Unity garbage-collection pauses (`Total: N ms (FindLiveObjects: … MarkObjects: …)`):
  /// the world thread stops for the whole pause, so every server-relayed update stalls.
  #[serde(default)]
  pub gc_pauses: Vec<GcPause>,
  /// Packets the server reports every 10 minutes (`Connections N ZDOS:N  sent:N recv:N`),
  /// accumulated into monotonic counters.
  #[serde(default)]
  pub packets_sent: u64,
  #[serde(default)]
  pub packets_received: u64,
  /// `Failed to send data k_EResult…` occurrences (a peer's socket gone while data was queued).
  #[serde(default)]
  pub send_failures: u64,
  /// Steam networking connection state transitions (`Got status changed msg
  /// k_ESteamNetworkingConnectionState_<State>`), counted per state.
  #[serde(default)]
  pub connection_states: BTreeMap<String, u64>,
  /// `Accepting connection k_EResult<Result>` per result (`OK` is a join that passed the
  /// password/ban/version checks).
  #[serde(default)]
  pub connection_results: BTreeMap<String, u64>,
  /// `Got handshake from client …` occurrences (connection attempts before any check).
  #[serde(default)]
  pub handshakes: u64,
  /// The network protocol version the server reports in `Network version check, their:N, mine:N`.
  #[serde(default)]
  pub network_version: Option<u32>,
  /// Joins refused because the client's network version differed from the server's.
  #[serde(default)]
  pub version_mismatches: u64,
  /// Unity asset unloads (`Unloading N unused Assets to reduce memory usage. Loaded Objects
  /// now: M.`): the object count after the last unload, and the unload count.
  #[serde(default)]
  pub loaded_objects: Option<u64>,
  #[serde(default)]
  pub asset_unloads: u64,
  /// `Server ID N` from the Steam game server init.
  #[serde(default)]
  pub server_id: Option<String>,
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

  /// Stores the result of an update check (called by `odin update`).
  pub fn record_update_check(current_build: &str, latest_build: &str) {
    let mut stats = WorldStats::load();
    stats.update = Some(UpdateCheck {
      current_build: current_build.to_string(),
      latest_build: latest_build.to_string(),
      available: current_build != latest_build,
      checked_at: Utc::now().timestamp(),
    });
    stats.save();
  }

  fn stat_world(&mut self) {
    let base = format!(
      "{}/worlds_local/{}",
      saves_directory(),
      fetch_var("WORLD", "Dedicated")
    );
    let bytes = match std::fs::read_dir(&base) {
      Ok(entries) => entries
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum(),
      Err(_) => match std::fs::metadata(format!("{base}.db")) {
        Ok(meta) => meta.len(),
        Err(_) => return,
      },
    };
    self.world_bytes = Some(bytes);
  }

  fn record_save(&mut self, kind: &str, ms: &str) {
    let Ok(ms) = ms.replace(',', "").parse::<f64>() else {
      return;
    };
    if kind == "save" {
      self.stat_world();
    }
    self.saves.push(Save {
      kind: kind.to_string(),
      seconds: ms / 1000.0,
      at: Utc::now().timestamp(),
    });
    if self.saves.len() > MAX_SAVES {
      self.saves.drain(..self.saves.len() - MAX_SAVES);
    }
  }

  fn record_gc_pause(&mut self, ms: &str) {
    let Ok(ms) = ms.parse::<f64>() else {
      return;
    };
    self.gc_pauses.push(GcPause {
      seconds: ms / 1000.0,
      at: Utc::now().timestamp(),
    });
    if self.gc_pauses.len() > MAX_GC_PAUSES {
      self.gc_pauses.drain(..self.gc_pauses.len() - MAX_GC_PAUSES);
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
      if let Some(p) = PACKETS.captures(line) {
        self.packets_sent += p[1].parse::<u64>().unwrap_or(0);
        self.packets_received += p[2].parse::<u64>().unwrap_or(0);
      }
    } else if let Some(c) = GC_PAUSE.captures(line) {
      self.record_gc_pause(&c[1]);
    } else if SEND_FAILED.is_match(line) {
      self.send_failures += 1;
    } else if let Some(c) = CONN_STATE.captures(line) {
      *self.connection_states.entry(c[1].to_string()).or_default() += 1;
    } else if let Some(c) = CONN_RESULT.captures(line) {
      *self.connection_results.entry(c[1].to_string()).or_default() += 1;
    } else if HANDSHAKE.is_match(line) {
      self.handshakes += 1;
    } else if let Some(c) = NET_VERSION.captures(line) {
      let mine: Option<u32> = c[2].parse().ok();
      self.network_version = mine;
      if c[1].parse::<u32>().ok() != mine {
        self.version_mismatches += 1;
      }
    } else if let Some(c) = UNLOAD.captures(line) {
      self.loaded_objects = c[1].parse().ok();
      self.asset_unloads += 1;
    } else if let Some(c) = SERVER_ID.captures(line) {
      self.server_id = Some(c[1].to_string());
    } else if let Some(c) = LOAD_CHUNKS.captures(line) {
      self.zdo_count = c[1].replace(',', "").parse().ok();
      self.stat_world();
    } else if let Some(c) = DAY.captures(line) {
      self.day = c[1].parse().ok();
    } else if let Some(c) = WORLD_SAVE.captures(line) {
      self.record_save("save", &c[1]);
    } else if let Some(c) = BACKUP.captures(line) {
      self.record_save("backup", &c[1]);
    } else if CONNECTED.is_match(line) {
      // First registration only: the game re-registers after a failure and logs the same line.
      if let (Some(start), None) = (self.boot_started, self.load_seconds) {
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
// Steam registration failures log `Game server connected failed`; the success line ends at
// `connected` (the tail delivers it with its newline).
static CONNECTED: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Game server connected\s*$").unwrap());
static RPC_TIMEOUT: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"ZRpc timeout detected").unwrap());
static WRONG_PASSWORD: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Peer (\d+) has wrong password").unwrap());
static GC_PAUSE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Total: ([\d.]+) ms \(FindLiveObjects:").unwrap());
static PACKETS: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"ZDOS:\d+\s+sent:(\d+) recv:(\d+)").unwrap());
static CONN_STATE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"Got status changed msg k_ESteamNetworkingConnectionState_(\w+)").unwrap()
});
static CONN_RESULT: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Accepting connection k_EResult(\w+)").unwrap());
static HANDSHAKE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Got handshake from client \d+").unwrap());
static NET_VERSION: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Network version check, their:(\d+), mine:(\d+)").unwrap());
static UNLOAD: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"Unloading \d+ unused Assets to reduce memory usage\. Loaded Objects now: (\d+)")
    .unwrap()
});
static SERVER_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Server ID (\d+)").unwrap());
static SEND_FAILED: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"Failed to send data k_EResult").unwrap());
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
    && !line.contains("FindLiveObjects:")
    && !line.contains("Failed to send data")
    && !line.contains("Got status changed msg")
    && !line.contains("Accepting connection")
    && !line.contains("Got handshake from client")
    && !line.contains("Network version check")
    && !line.contains("Loaded Objects now:")
    && !line.contains("Server ID ")
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
    // a failed Steam registration has the success line as its prefix
    assert!(!s.apply("09/09/2026 23:39:50: Game server connected failed\n"));
    assert!(s.load_seconds.is_none());
    // lines arrive from the tail with their newline
    assert!(s.apply("09/09/2026 23:40:04: Game server connected\n"));
    assert!(s.load_seconds.is_some_and(|l| (52.0..54.0).contains(&l)));
    // re-registration after a failure logs the same line; load time stays the first one
    s.boot_started = Some(Utc::now().timestamp() - 900);
    assert!(s.apply("09/09/2026 23:55:04: Game server connected\n"));
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
  fn parses_gc_packets_and_send_failures() {
    let mut s = WorldStats::default();
    assert!(s.apply("09/13/2026 21:19:57:  Connections 5 ZDOS:454072  sent:3444 recv:582"));
    assert!(s.apply("09/13/2026 21:29:58:  Connections 5 ZDOS:454035  sent:1023 recv:732"));
    assert_eq!(s.packets_sent, 4467);
    assert_eq!(s.packets_received, 1314);
    assert!(s.apply(
      "Total: 1492.614768 ms (FindLiveObjects: 130.303470 ms CreateObjectMapping: 101.198868 ms MarkObjects: 1253.295171 ms  DeleteObjects: 7.816326 ms)\n"
    ));
    assert_eq!(s.gc_pauses.len(), 1);
    assert!((s.gc_pauses[0].seconds - 1.492614768).abs() < 1e-9);
    assert!(s.apply("09/13/2026 17:26:59: Failed to send data k_EResultNoConnection"));
    assert!(s.apply("09/13/2026 17:26:59: Failed to send data k_EResultNoConnection"));
    assert_eq!(s.send_failures, 2);
    assert!(!s.apply("09/13/2026 17:00:00: Sending message to save player profiles"));
  }

  #[test]
  fn parses_connection_and_unload_lines() {
    let mut s = WorldStats::default();
    assert!(s.apply("09/13/2026 19:09:14: Server ID 90071992547409920"));
    assert_eq!(s.server_id.as_deref(), Some("90071992547409920"));
    assert!(s.apply("09/13/2026 19:09:42: Got handshake from client 76561190000000000"));
    assert!(s.apply("09/13/2026 19:09:42: Network version check, their:40, mine:40"));
    assert!(s.apply("09/13/2026 19:09:43: Network version check, their:39, mine:40"));
    assert_eq!(s.network_version, Some(40));
    assert_eq!(s.version_mismatches, 1);
    assert!(s.apply(
      "09/13/2026 19:09:42: Got status changed msg k_ESteamNetworkingConnectionState_Connecting"
    ));
    assert!(s.apply(
      "09/13/2026 19:09:42: Got status changed msg k_ESteamNetworkingConnectionState_Connected"
    ));
    assert!(s.apply(
      "09/13/2026 19:09:42: Got status changed msg k_ESteamNetworkingConnectionState_Connected\n"
    ));
    assert!(s.apply("09/13/2026 19:09:42: Accepting connection k_EResultOK"));
    assert!(s.apply("09/13/2026 19:09:50: Accepting connection k_EResultBanned"));
    assert_eq!(s.handshakes, 1);
    assert_eq!(s.connection_states["Connecting"], 1);
    assert_eq!(s.connection_states["Connected"], 2);
    assert_eq!(s.connection_results["OK"], 1);
    assert_eq!(s.connection_results["Banned"], 1);
    assert!(
      s.apply("Unloading 6 unused Assets to reduce memory usage. Loaded Objects now: 146522.")
    );
    assert_eq!(s.loaded_objects, Some(146522));
    assert_eq!(s.asset_unloads, 1);
    assert!(!s.apply("Unloading 4 Unused Serialized files (Serialized files now loaded: 8)"));
  }

  #[test]
  fn keeps_bounded_gc_history() {
    let mut s = WorldStats::default();
    for _ in 0..(MAX_GC_PAUSES + 10) {
      s.record_gc_pause("500");
    }
    assert_eq!(s.gc_pauses.len(), MAX_GC_PAUSES);
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
