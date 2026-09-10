use crate::fetch_info;
use odin::log_filters::{PlayerList, WorldStats};
use shared::system::collect_system_metrics;

const SAVE_BUCKETS: [f64; 11] = [0.1, 0.25, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 30.0, 60.0];

fn escape_prom_label_value(value: &str) -> String {
  value
    .replace('\\', "\\\\")
    .replace('"', "\\\"")
    .replace('\n', "\\n")
}

pub fn invoke() -> String {
  let info = fetch_info();
  let sys = collect_system_metrics();
  let labels = format!(
    "{{name=\"{name}\", version=\"{version}\", map=\"{map}\"}}",
    name = escape_prom_label_value(&info.name),
    version = escape_prom_label_value(&info.version),
    map = escape_prom_label_value(&info.map)
  );
  let players = PlayerList::online().into_iter().flat_map(|p| {
    let player = escape_prom_label_value(&p.name);
    [
      format!("valheim_player_online{{player=\"{player}\"}} 1"),
      format!(
        "valheim_player_joined_timestamp_seconds{{player=\"{player}\"}} {}",
        p.joined_at
      ),
    ]
  });
  let content = [
    format!(
      "valheim_online{labels} {online}",
      labels = labels,
      online = info.online as i32
    ),
    format!(
      "valheim_current_player_count{labels} {players}",
      labels = labels,
      players = info.players
    ),
    format!(
      "valheim_max_player_count{labels} {players}",
      labels = labels,
      players = info.max_players
    ),
    format!(
      "valheim_bepinex_installed{labels} {bepinex_installed}",
      labels = labels,
      bepinex_installed = info.bepinex.enabled as i32
    ),
    // System metrics (no labels beyond server identity)
    format!(
      "valheim_sys_memory_total_bytes {:.0}",
      sys.total_memory_bytes
    ),
    format!("valheim_sys_memory_used_bytes {:.0}", sys.used_memory_bytes),
    format!("valheim_sys_swap_total_bytes {:.0}", sys.total_swap_bytes),
    format!("valheim_sys_swap_used_bytes {:.0}", sys.used_swap_bytes),
    format!("valheim_sys_disk_total_bytes {:.0}", sys.total_disk_bytes),
    format!(
      "valheim_sys_disk_available_bytes {:.0}",
      sys.available_disk_bytes
    ),
    format!("valheim_sys_cpu_logical_count {}", sys.cpu_num_logical),
    format!(
      "valheim_sys_load_average {{window=\"1m\"}} {:.2}",
      sys.load_average_one
    ),
    format!(
      "valheim_sys_load_average {{window=\"5m\"}} {:.2}",
      sys.load_average_five
    ),
    format!(
      "valheim_sys_load_average {{window=\"15m\"}} {:.2}",
      sys.load_average_fifteen
    ),
  ];
  format!(
    "{}\n",
    content
      .into_iter()
      .chain(players)
      .chain(world_metrics(&WorldStats::load()))
      .collect::<Vec<_>>()
      .join("\n")
  )
}

/// Metrics Odin parses from the server log into `world.stats`.
fn world_metrics(world: &WorldStats) -> Vec<String> {
  let mut out = Vec::new();
  if let Some(n) = world.zdo_count {
    out.push(format!("valheim_world_zdo_count {n}"));
  }
  if let Some(n) = world.day {
    out.push(format!("valheim_world_day {n}"));
  }
  if let Some(s) = world.load_seconds {
    out.push(format!("valheim_world_load_seconds {s}"));
  }
  out.push(format!("valheim_rpc_timeouts_total {}", world.rpc_timeouts));
  for w in &world.wrong_password {
    out.push(format!(
      "valheim_wrong_password_total{{steam_id=\"{}\", name=\"{}\"}} {}",
      w.steam_id,
      escape_prom_label_value(&w.name),
      w.count
    ));
  }
  for kind in ["save", "backup"] {
    let saves: Vec<_> = world.saves.iter().filter(|s| s.kind == kind).collect();
    if let Some(last) = saves.last() {
      out.push(format!(
        "valheim_world_last_save_seconds{{type=\"{kind}\"}} {}",
        last.seconds
      ));
      out.push(format!(
        "valheim_world_last_save_timestamp_seconds{{type=\"{kind}\"}} {}",
        last.at
      ));
    }
  }
  if !world.saves.is_empty() {
    out.push("# TYPE valheim_world_save_duration_seconds histogram".to_string());
    for kind in ["save", "backup"] {
      let seconds: Vec<f64> = world
        .saves
        .iter()
        .filter(|s| s.kind == kind)
        .map(|s| s.seconds)
        .collect();
      if seconds.is_empty() {
        continue;
      }
      for le in SAVE_BUCKETS {
        let n = seconds.iter().filter(|&&s| s <= le).count();
        out.push(format!(
          "valheim_world_save_duration_seconds_bucket{{type=\"{kind}\", le=\"{le}\"}} {n}"
        ));
      }
      out.push(format!(
        "valheim_world_save_duration_seconds_bucket{{type=\"{kind}\", le=\"+Inf\"}} {}",
        seconds.len()
      ));
      out.push(format!(
        "valheim_world_save_duration_seconds_sum{{type=\"{kind}\"}} {}",
        seconds.iter().sum::<f64>()
      ));
      out.push(format!(
        "valheim_world_save_duration_seconds_count{{type=\"{kind}\"}} {}",
        seconds.len()
      ));
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use odin::log_filters::{Save, WrongPassword};

  #[test]
  fn world_metrics_render() {
    let mut world = WorldStats::default();
    world.zdo_count = Some(76806);
    world.day = Some(4);
    world.load_seconds = Some(52.0);
    world.rpc_timeouts = 1;
    world.wrong_password = vec![WrongPassword {
      steam_id: "76561190000000000".into(),
      name: "Viking".into(),
      count: 2,
    }];
    world.saves = vec![
      Save {
        kind: "save".into(),
        seconds: 2.844,
        at: 1_789_000_000,
      },
      Save {
        kind: "backup".into(),
        seconds: 12.5,
        at: 1_789_000_100,
      },
    ];
    let lines = world_metrics(&world);
    let text = lines.join("\n");
    assert!(text.contains("valheim_world_zdo_count 76806"));
    assert!(text.contains("valheim_world_day 4"));
    assert!(text.contains("valheim_world_load_seconds 52"));
    assert!(text.contains("valheim_rpc_timeouts_total 1"));
    assert!(text
      .contains("valheim_wrong_password_total{steam_id=\"76561190000000000\", name=\"Viking\"} 2"));
    assert!(text.contains("valheim_world_last_save_seconds{type=\"save\"} 2.844"));
    assert!(text.contains("valheim_world_last_save_timestamp_seconds{type=\"backup\"} 1789000100"));
    assert!(text.contains("valheim_world_save_duration_seconds_bucket{type=\"save\", le=\"3\"} 1"));
    assert!(text.contains("valheim_world_save_duration_seconds_bucket{type=\"save\", le=\"2\"} 0"));
    assert!(
      text.contains("valheim_world_save_duration_seconds_bucket{type=\"backup\", le=\"+Inf\"} 1")
    );
    assert!(text.contains("valheim_world_save_duration_seconds_count{type=\"save\"} 1"));
  }
}
