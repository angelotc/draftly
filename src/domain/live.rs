use serde_json::Value;
use std::collections::HashMap;

pub fn live_position(pos: &str) -> &'static str {
    match pos.to_ascii_uppercase().as_str() {
        "TOP" => "top",
        "JUNGLE" => "jungle",
        "MIDDLE" => "middle",
        "BOTTOM" => "bottom",
        "UTILITY" => "support",
        _ => "",
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LivePlayer {
    pub enemy: bool,
    pub lane: String,
    pub champion: String,
    pub level: i32,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    pub cs: i32,
    pub items: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveGame {
    pub champion: String,
    pub role: String,
    pub level: i32,
    pub gold: f64,
    pub items: Vec<i32>,
    pub game_time: f64,
    pub allies: HashMap<String, String>,
    pub enemies: HashMap<String, String>,
    pub players: Vec<LivePlayer>,
}

impl LiveGame {
    /// Compare ignoring `game_time` (and tiny gold fluctuations if desired) so the 2-second poll
    /// doesn't trigger full rebuilds when only the clock ticked.
    pub fn same_static_state(&self, other: &Self) -> bool {
        self.champion == other.champion
            && self.role == other.role
            && self.level == other.level
            && self.items == other.items
            && self.allies == other.allies
            && self.enemies == other.enemies
            && (self.gold as i64) == (other.gold as i64)
            && self.enemy_state() == other.enemy_state()
    }

    /// (lane, level, items) for each enemy player; what can change the threat multipliers.
    fn enemy_state(&self) -> Vec<(&str, i32, &[i32])> {
        self.players
            .iter()
            .filter(|p| p.enemy)
            .map(|p| (p.lane.as_str(), p.level, p.items.as_slice()))
            .collect()
    }
}

pub fn parse_live(data: &Value) -> Option<LiveGame> {
    let ap = data.get("activePlayer")?;
    let players = data.get("allPlayers")?.as_array()?;
    let me_id = ap
        .get("riotId")
        .or_else(|| ap.get("summonerName"))
        .and_then(|v| v.as_str())?;
    let me = players.iter().find(|p| {
        p.get("riotId")
            .or_else(|| p.get("summonerName"))
            .and_then(|v| v.as_str())
            == Some(me_id)
    })?;
    let team = me.get("team").and_then(|v| v.as_str());
    let mut allies = HashMap::new();
    let mut enemies = HashMap::new();

    let mut live_players = Vec::new();

    for p in players {
        let is_me = std::ptr::eq(p, me);
        let lane = live_position(p.get("position").and_then(|v| v.as_str()).unwrap_or(""));
        let cname = p
            .get("championName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_ally = p.get("team").and_then(|v| v.as_str()) == team;
        let int = |v: Option<&Value>| v.and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let scores = p.get("scores");
        live_players.push(LivePlayer {
            enemy: !is_me && !is_ally,
            lane: lane.to_string(),
            champion: cname.clone(),
            level: int(p.get("level")),
            kills: int(scores.and_then(|s| s.get("kills"))),
            deaths: int(scores.and_then(|s| s.get("deaths"))),
            assists: int(scores.and_then(|s| s.get("assists"))),
            cs: int(scores.and_then(|s| s.get("creepScore"))),
            items: p
                .get("items")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|i| int(i.get("itemID")))
                        .filter(|&id| id != 0)
                        .collect()
                })
                .unwrap_or_default(),
        });
        if is_me || lane.is_empty() {
            continue;
        }
        if is_ally {
            allies.insert(lane.to_string(), cname);
        } else {
            enemies.insert(lane.to_string(), cname);
        }
    }

    let mut item_rows: Vec<(i64, i32)> = me
        .get("items")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| {
                    let id = i.get("itemID").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    if id == 0 {
                        return None;
                    }
                    let slot = i.get("slot").and_then(|v| v.as_i64()).unwrap_or(0);
                    Some((slot, id))
                })
                .collect()
        })
        .unwrap_or_default();
    item_rows.sort_by_key(|&(slot, _)| slot);
    let items: Vec<i32> = item_rows.into_iter().map(|(_, id)| id).collect();

    let champion = me
        .get("championName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let role = live_position(me.get("position").and_then(|v| v.as_str()).unwrap_or("")).to_string();
    let level = ap
        .get("level")
        .or_else(|| me.get("level"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let gold = ap
        .get("currentGold")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let game_time = data
        .get("gameData")
        .and_then(|g| g.get("gameTime"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    Some(LiveGame {
        champion,
        role,
        level,
        gold,
        items,
        game_time,
        allies,
        enemies,
        players: live_players,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_parse_live_fixture() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live.json");
        let txt = std::fs::read_to_string(p).unwrap();
        let val: Value = serde_json::from_str(&txt).unwrap();
        let g = parse_live(&val).unwrap();
        assert!(!g.champion.is_empty());
        assert_eq!(g.players.len(), 10);
        assert!(g.players.iter().all(|p| p.kills == 0));
    }

    #[test]
    fn test_parse_live_scores_fixture() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live_scores.json");
        let txt = std::fs::read_to_string(p).unwrap();
        let val: Value = serde_json::from_str(&txt).unwrap();
        let g = parse_live(&val).unwrap();
        assert_eq!(g.players.len(), 10);
        assert_eq!(g.players.iter().filter(|p| p.enemy).count(), 5);
        assert!(g.players.iter().all(|p| p.level > 0 && !p.items.is_empty()));
        let ahri = g.players.iter().find(|p| p.champion == "Ahri").unwrap();
        assert!(ahri.enemy && ahri.kills == 8 && ahri.cs > 0);
    }

    #[test]
    fn test_same_static_state_sees_enemy_level_and_items() {
        let a = LiveGame {
            champion: "A".into(),
            role: "top".into(),
            level: 5,
            gold: 0.0,
            items: vec![],
            game_time: 10.0,
            allies: Default::default(),
            enemies: Default::default(),
            players: vec![LivePlayer {
                enemy: true,
                lane: "middle".into(),
                level: 5,
                ..Default::default()
            }],
        };
        let mut b = a.clone();
        b.game_time = 20.0;
        assert!(a.same_static_state(&b));
        b.players[0].level = 6;
        assert!(!a.same_static_state(&b));
        b.players[0].level = 5;
        b.players[0].items = vec![3157];
        assert!(!a.same_static_state(&b));
    }
}
