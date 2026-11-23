//! Per-enemy "threat" multiplier from live item gold and level.
//! All constants are provisional until tuned on recorded games.

use std::collections::HashMap;

use super::live::LivePlayer;

pub const GOLD_WEIGHT: f64 = 0.7;
pub const LEVEL_WEIGHT: f64 = 0.3;
pub const FADE_SECS: f64 = 300.0;
pub const MIN_MULT: f64 = 0.5;
pub const MAX_MULT: f64 = 2.0;
pub const FED: f64 = 1.3;
pub const BEHIND: f64 = 0.7;
pub const FLOOR_WEIGHT: f64 = 0.3;

fn mean(xs: impl Iterator<Item = f64>) -> f64 {
    let (s, n) = xs.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    if n == 0 {
        0.0
    } else {
        s / n as f64
    }
}

fn raw_threat(players: &[LivePlayer], item_gold: &impl Fn(i32) -> i32) -> Vec<(usize, f64)> {
    let gold: Vec<f64> = players
        .iter()
        .map(|p| p.items.iter().map(|&i| item_gold(i) as f64).sum())
        .collect();
    let mean_gold = mean(gold.iter().copied());
    let mean_level = mean(players.iter().map(|p| p.level as f64));
    let (gw, lw) = match (mean_gold > 0.0, mean_level > 0.0) {
        (true, true) => (GOLD_WEIGHT, LEVEL_WEIGHT),
        (true, false) => (1.0, 0.0),
        (false, true) => (0.0, 1.0),
        (false, false) => return Vec::new(),
    };
    let strength = |i: usize| {
        let mut s = 0.0;
        if gw > 0.0 {
            s += gw * gold[i] / mean_gold;
        }
        if lw > 0.0 {
            s += lw * players[i].level as f64 / mean_level;
        }
        s
    };
    let enemies: Vec<(usize, f64)> = (0..players.len())
        .filter(|&i| players[i].enemy)
        .map(|i| (i, strength(i)))
        .collect();
    let m = mean(enemies.iter().map(|e| e.1));
    if m <= 0.0 {
        return Vec::new();
    }
    enemies.into_iter().map(|(i, s)| (i, s / m)).collect()
}

/// Unrounded, clamped multiplier per enemy lane.
fn threat_unrounded(
    players: &[LivePlayer],
    game_time: f64,
    item_gold: impl Fn(i32) -> i32,
) -> HashMap<String, f64> {
    let t = (game_time / FADE_SECS).clamp(0.0, 1.0);
    raw_threat(players, &item_gold)
        .into_iter()
        .filter(|&(i, _)| !players[i].lane.is_empty())
        .map(|(i, raw)| {
            let mult = (1.0 + (raw - 1.0) * t).clamp(MIN_MULT, MAX_MULT);
            (players[i].lane.clone(), mult)
        })
        .collect()
}

/// Threat multiplier per enemy lane (1.0 = average enemy). Empty map = no usable data.
pub fn threat(
    players: &[LivePlayer],
    game_time: f64,
    item_gold: impl Fn(i32) -> i32,
) -> HashMap<String, f64> {
    threat_unrounded(players, game_time, item_gold)
        .into_iter()
        .map(|(k, v)| (k, (v * 10.0).round() / 10.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gold(id: i32) -> i32 {
        id // item id doubles as its price in tests
    }

    fn team(enemy_items: [Vec<i32>; 5]) -> Vec<LivePlayer> {
        let lanes = ["top", "jungle", "middle", "bottom", "support"];
        let mut v: Vec<LivePlayer> = (0..5)
            .map(|_| LivePlayer {
                level: 10,
                items: vec![3000],
                ..Default::default()
            })
            .collect();
        for (i, items) in enemy_items.into_iter().enumerate() {
            v.push(LivePlayer {
                enemy: true,
                lane: lanes[i].into(),
                level: 10,
                items,
                ..Default::default()
            });
        }
        v
    }

    #[test]
    fn even_game_is_all_one() {
        let p = team(std::array::from_fn(|_| vec![3000]));
        let m = threat(&p, 900.0, gold);
        assert_eq!(m.len(), 5);
        assert!(m.values().all(|&v| v == 1.0));
    }

    #[test]
    fn double_gold_enemy_is_fed() {
        let p = team([vec![6000], vec![3000], vec![3000], vec![3000], vec![3000]]);
        let m = threat(&p, 900.0, gold);
        assert!(m["top"] > 1.3, "{m:?}");
        assert!(m["middle"] < 1.0);
    }

    #[test]
    fn clamps_hold() {
        let p = team([vec![100000], vec![1], vec![1], vec![1], vec![1]]);
        let m = threat(&p, 900.0, gold);
        assert!(m.values().all(|&v| (MIN_MULT..=MAX_MULT).contains(&v)));
        assert_eq!(m["top"], MAX_MULT);
        assert_eq!(m["jungle"], MIN_MULT);
    }

    #[test]
    fn fades_in() {
        let p = team([vec![6000], vec![3000], vec![3000], vec![3000], vec![3000]]);
        assert!(threat(&p, 0.0, gold).values().all(|&v| v == 1.0));
        let full = threat_unrounded(&p, 300.0, gold)["top"] - 1.0;
        let half = threat_unrounded(&p, 150.0, gold)["top"] - 1.0;
        assert!(full > 0.0);
        assert!((half - full / 2.0).abs() < 1e-9);
        let late = threat_unrounded(&p, 3000.0, gold)["top"] - 1.0;
        assert!((late - full).abs() < 1e-9);
    }

    #[test]
    fn empty_and_degenerate_inputs() {
        assert!(threat(&[], 600.0, gold).is_empty());
        let allies_only = vec![LivePlayer {
            level: 5,
            ..Default::default()
        }];
        assert!(threat(&allies_only, 600.0, gold).is_empty());
        let zeros = vec![LivePlayer {
            enemy: true,
            lane: "top".into(),
            ..Default::default()
        }];
        assert!(threat(&zeros, 600.0, gold).is_empty());
    }

    #[test]
    fn missing_gold_falls_back_to_level() {
        let mut p = team(std::array::from_fn(|_| vec![]));
        p[5].level = 18;
        let m = threat(&p, 900.0, gold);
        assert!(m["top"] > 1.0 && m["jungle"] < 1.0);
    }

    #[test]
    fn empty_lane_counts_in_means_but_not_output() {
        let mut p = team(std::array::from_fn(|_| vec![3000]));
        p[9].lane.clear();
        p[9].items = vec![9000];
        let m = threat(&p, 900.0, gold);
        assert_eq!(m.len(), 4);
        assert!(m["top"] < 1.0);
    }
}
