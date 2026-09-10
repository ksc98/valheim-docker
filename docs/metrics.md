# Metrics

Huginn serves Prometheus metrics at `http://<host>:<HTTP_PORT>/metrics` (default port `3000`). Server metrics come from the Steam A2S query, player and world metrics from Odin's log tracking, system metrics from the container's view of the host. Everything is a gauge except the `_total` counters and the save-duration histogram.

## Server

Labels on every server metric: `name` (server name), `version` (Steam server version tag), `map` (world name).

| Metric                         | Labels                 | Description                                                        |
| ------------------------------ | ---------------------- | ------------------------------------------------------------------ |
| `valheim_online`               | `name`,`version`,`map` | `1` when the server answers the Steam query, `0` when it does not. |
| `valheim_current_player_count` | `name`,`version`,`map` | Players connected.                                                 |
| `valheim_max_player_count`     | `name`,`version`,`map` | Player slots.                                                      |
| `valheim_bepinex_installed`    | `name`,`version`,`map` | `1` when BepInEx is installed, else `0`.                           |

## Players

One series per player currently online. Names come from the server log (`Got character ZDOID from <name>`), which Odin tracks in `player.list`; Valheim does not publish names over the Steam query. Series disappear when the player leaves.

| Metric                                    | Labels   | Description                                                   |
| ----------------------------------------- | -------- | ------------------------------------------------------------- |
| `valheim_player_online`                   | `player` | Always `1` while the character is online.                     |
| `valheim_player_joined_timestamp_seconds` | `player` | Unix time the player joined. Kept across deaths and respawns. |

Time in game: `time() - valheim_player_joined_timestamp_seconds`.

## World

Parsed by Odin from the server log into `world.stats`, which survives server restarts (only the load time is per boot).

| Metric                                                | Labels             | Description                                                                                                                    |
| ----------------------------------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| `valheim_world_zdo_count`                             |                    | Objects (ZDOs) in the world: the count loaded at boot, then the server's report every 10 minutes.                              |
| `valheim_world_day`                                   |                    | In-game day, updated when the players sleep through a night.                                                                   |
| `valheim_world_load_seconds`                          |                    | Seconds from `odin start` to `Game server connected`.                                                               |
| `valheim_world_last_save_seconds`                     | `type`             | Duration of the latest `save` (world autosave) or `backup` (world auto backup).                                                 |
| `valheim_world_last_save_timestamp_seconds`           | `type`             | Unix time the latest `save` / `backup` finished.                                                                               |
| `valheim_world_save_duration_seconds` (histogram)     | `type`, `le`       | The last 500 save durations (`_bucket`, `_sum`, `_count`); buckets 0.1 s to 60 s. Feeds a Grafana heatmap or quantiles.       |
| `valheim_rpc_timeouts_total`                          |                    | `ZRpc timeout detected` occurrences (a peer stopped answering).                                                                |
| `valheim_wrong_password_total`                        | `steam_id`, `name` | Rejected joins per Steam id; `name` is the Steam display name when that id has joined before, else empty.                      |

## System

| Metric                             | Labels   | Description                                    |
| ---------------------------------- | -------- | ---------------------------------------------- |
| `valheim_sys_memory_total_bytes`   |          | Total memory.                                  |
| `valheim_sys_memory_used_bytes`    |          | Memory in use.                                 |
| `valheim_sys_swap_total_bytes`     |          | Total swap.                                    |
| `valheim_sys_swap_used_bytes`      |          | Swap in use.                                   |
| `valheim_sys_disk_total_bytes`     |          | Total size of all mounted disks.               |
| `valheim_sys_disk_available_bytes` |          | Free space across all mounted disks.           |
| `valheim_sys_cpu_logical_count`    |          | Logical CPUs.                                  |
| `valheim_sys_load_average`         | `window` | Load average; `window` is `1m`, `5m` or `15m`. |

## Example

```
valheim_online{name="My Server", version="g=1.0.7,n=39", map="Dedicated"} 1
valheim_current_player_count{name="My Server", version="g=1.0.7,n=39", map="Dedicated"} 2
valheim_max_player_count{name="My Server", version="g=1.0.7,n=39", map="Dedicated"} 10
valheim_bepinex_installed{name="My Server", version="g=1.0.7,n=39", map="Dedicated"} 0
valheim_sys_memory_total_bytes 8317225140
valheim_sys_memory_used_bytes 6340026040
valheim_sys_swap_total_bytes 0
valheim_sys_swap_used_bytes 0
valheim_sys_disk_total_bytes 833209548800
valheim_sys_disk_available_bytes 450985697280
valheim_sys_cpu_logical_count 4
valheim_sys_load_average {window="1m"} 0.98
valheim_sys_load_average {window="5m"} 0.94
valheim_sys_load_average {window="15m"} 1.01
valheim_player_online{player="Viking"} 1
valheim_player_joined_timestamp_seconds{player="Viking"} 1789020710
valheim_world_zdo_count 76806
valheim_world_day 4
valheim_world_load_seconds 52
valheim_rpc_timeouts_total 0
valheim_world_last_save_seconds{type="save"} 2.844
valheim_world_last_save_timestamp_seconds{type="save"} 1789021771
# TYPE valheim_world_save_duration_seconds histogram
valheim_world_save_duration_seconds_bucket{type="save", le="0.1"} 0
...
valheim_world_save_duration_seconds_bucket{type="save", le="+Inf"} 6
valheim_world_save_duration_seconds_sum{type="save"} 17.9
valheim_world_save_duration_seconds_count{type="save"} 6
```

Scrape it like any other target (Prometheus `static_configs`, or a Kubernetes `ServiceMonitor` on the Huginn port). A Grafana dashboard walkthrough is in [discussion #330](https://github.com/mbround18/valheim-docker/discussions/330).
