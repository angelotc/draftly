use serde_json::Value;
use std::collections::HashMap;

use crate::domain::lcu::{DraftState, parse_session};
use crate::domain::review::*;
use crate::domain::scoring::{Comfort, Recommendation};

use super::recommender::Evaluation;

/// The board the player decided on, rebuilt from a draft's saved LCU sessions (oldest first): the
/// last session with no lock-in that precedes the first session where the player had locked in.
/// None when the player never locked in, or the very first saved session already had the lock.
pub fn decision_board(sessions: &[Value]) -> Option<DraftState> {
    let boards: Vec<DraftState> = sessions.iter().map(parse_session).collect();
    let first_lock = boards.iter().position(|b| b.my_locked != 0)?;
    boards[..first_lock].iter().rev().find(|b| b.my_locked == 0).cloned()
}

fn sum_of(r: &Recommendation, labels: &[&str]) -> f64 {
    r.contributions
        .iter()
        .filter(|c| labels.contains(&c.label.as_str()))
        .map(|c| c.weighted)
        .sum()
}

/// Build the review for a board that was scored with `Recommender::evaluate_full`.
/// `draft` is the board when the player decided (the player's own pick is not on it).
/// Blind mode follows the Picks tab: it applies when the evaluation produced a blind ranking.
pub fn build_review(
    ev: &Evaluation,
    comfort: &HashMap<i32, Comfort>,
    my_champ: i32,
    tier: &str,
    source: &str,
) -> ReviewRecord {
    let blind_mode = !ev.blind.is_empty();
    let by_id: HashMap<i32, &Recommendation> = ev.recs.iter().map(|r| (r.champion_id, r)).collect();

    // (champion, score, comfort, vs, with, base) in ranked order.
    let ranked: Vec<(i32, f64, f64, f64, f64, f64)> = if blind_mode {
        ev.blind
            .iter()
            .map(|b| {
                let base = by_id.get(&b.champion_id).map_or(0.0, |r| r.base);
                (b.champion_id, b.score, b.comfort, 0.0, b.synergy, base)
            })
            .collect()
    } else {
        ev.recs
            .iter()
            .map(|r| {
                (
                    r.champion_id,
                    r.score,
                    sum_of(r, &["you", "mastery"]),
                    sum_of(r, &["vs"]),
                    sum_of(r, &["with"]),
                    r.base,
                )
            })
            .collect()
    };

    let picked_idx = ranked.iter().position(|r| r.0 == my_champ);
    let options = ranked
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < REVIEW_TOP || Some(*i) == picked_idx)
        .map(|(i, &(cid, score, comfort_sum, vs, with, base))| {
            let c = comfort.get(&cid);
            ReviewOption {
                champion_id: cid,
                rank: i + 1,
                score,
                comfort: comfort_sum,
                anyone: score - comfort_sum,
                vs,
                with,
                base,
                games: c.map(|c| c.games).filter(|&g| g > 0),
                wins: c.filter(|c| c.games > 0).map(|c| c.wins),
                mastery: c.map(|c| c.mastery).filter(|&m| m > 0),
            }
        })
        .collect();

    ReviewRecord {
        version: REVIEW_VERSION,
        source: source.to_string(),
        role: ev.role.clone(),
        tier: tier.to_string(),
        my_champ,
        mode: if blind_mode { MODE_BLIND } else { MODE_COUNTER }.to_string(),
        allies: review_allies(&ev.draft),
        enemies: review_enemies(ev),
        bans: {
            let mut b: Vec<i32> = ev.draft.bans.iter().copied().collect();
            b.sort_unstable();
            b
        },
        options,
        total: ranked.len(),
        picked_rank: picked_idx.map(|i| i + 1),
    }
}

fn review_allies(draft: &DraftState) -> Vec<ReviewAlly> {
    draft
        .allies
        .iter()
        .filter(|s| !s.is_me && s.champion_id != 0)
        .map(|s| ReviewAlly {
            champion_id: s.champion_id,
            position: s.position.clone(),
            locked: s.locked && !s.tentative,
        })
        .collect()
}

fn review_enemies(ev: &Evaluation) -> Vec<ReviewEnemy> {
    ev.draft
        .enemies
        .iter()
        .map(|&cid| ReviewEnemy {
            champion_id: cid,
            lane: ev.lanes.get(&cid).cloned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::lcu::Slot;
    use serde_json::json;
    use crate::domain::scoring::{Blind, Contribution};

    fn contrib(label: &str, weighted: f64) -> Contribution {
        Contribution {
            label: label.into(),
            champion_id: 9,
            lane: "support".into(),
            delta: 0.0,
            games: 10,
            weighted,
            tentative: false,
        }
    }

    fn rec(cid: i32, base: f64, contribs: Vec<Contribution>) -> Recommendation {
        let score = base + contribs.iter().map(|c| c.weighted).sum::<f64>();
        Recommendation { champion_id: cid, score, base, contributions: contribs }
    }

    fn evaluation(recs: Vec<Recommendation>, blind: Vec<Blind>) -> Evaluation {
        let draft = DraftState {
            allies: vec![
                Slot { cell_id: 0, champion_id: 7, position: "bottom".into(), tentative: false, locked: true, is_me: false },
                Slot { cell_id: 1, champion_id: 8, position: "jungle".into(), tentative: true, locked: false, is_me: false },
                Slot { cell_id: 2, champion_id: 412, position: "support".into(), tentative: false, locked: true, is_me: true },
            ],
            enemies: vec![51],
            bans: [3, 1].into_iter().collect(),
            ..Default::default()
        };
        Evaluation {
            draft,
            role: "support".into(),
            recs,
            lanes: HashMap::from([(51, "bottom".to_string())]),
            enemy_rates: HashMap::new(),
            blind,
        }
    }

    #[test]
    fn counter_review_keeps_top_options_and_the_pick() {
        let mut recs: Vec<Recommendation> = (1..=20)
            .map(|i| rec(100 + i, 0.0, vec![contrib("vs", 20.0 - i as f64), contrib("with", 0.5)]))
            .collect();
        // the pick is ranked 15th and carries a comfort bonus
        recs[14] = rec(199, 0.0, vec![contrib("vs", 1.0), contrib("you", 0.4), contrib("mastery", 0.2)]);
        recs.sort_by(|a, b| b.score.total_cmp(&a.score));
        let ev = evaluation(recs, vec![]);
        let comfort = HashMap::from([(199, Comfort { games: 5, wins: 3, mastery: 20000, ..Default::default() })]);
        let r = build_review(&ev, &comfort, 199, "master_plus", SOURCE_LIVE);

        assert_eq!(r.mode, MODE_COUNTER);
        assert_eq!(r.total, 20);
        let picked_rank = r.picked_rank.unwrap();
        assert!(picked_rank > REVIEW_TOP);
        assert_eq!(r.options.len(), REVIEW_TOP + 1);
        let picked = r.options.iter().find(|o| o.champion_id == 199).unwrap();
        assert_eq!(picked.rank, picked_rank);
        assert!((picked.comfort - 0.6).abs() < 1e-9);
        assert!((picked.anyone - (picked.score - 0.6)).abs() < 1e-9);
        assert!((picked.vs - 1.0).abs() < 1e-9);
        assert_eq!((picked.games, picked.wins, picked.mastery), (Some(5), Some(3), Some(20000)));
        assert_eq!(r.options[0].rank, 1);
        assert_eq!(r.options[0].games, None);
        assert_eq!(r.bans, vec![1, 3]);
        // allies exclude the player and keep hover state; enemies carry their lane
        assert_eq!(
            r.allies,
            vec![
                ReviewAlly { champion_id: 7, position: "bottom".into(), locked: true },
                ReviewAlly { champion_id: 8, position: "jungle".into(), locked: false },
            ]
        );
        assert_eq!(r.enemies, vec![ReviewEnemy { champion_id: 51, lane: Some("bottom".into()) }]);
    }

    #[test]
    fn blind_review_uses_the_blind_ranking() {
        let recs = vec![
            rec(1, 1.0, vec![contrib("with", 0.3), contrib("you", 0.5)]),
            rec(2, 0.5, vec![contrib("with", 0.1)]),
        ];
        let blind = |cid: i32, score: f64, synergy: f64, comfort: f64| Blind {
            champion_id: cid,
            score,
            value: 0.0,
            mean: 0.0,
            tail: 0.0,
            coverage: 1.0,
            counters: vec![],
            synergy,
            comfort,
            known_tail: 0.0,
        };
        let ev = evaluation(recs, vec![blind(2, 2.0, 0.1, 0.0), blind(1, 1.5, 0.3, 0.5)]);
        let r = build_review(&ev, &HashMap::new(), 1, "master_plus", SOURCE_REPLAYED);
        assert_eq!(r.mode, MODE_BLIND);
        assert!(r.is_replayed());
        assert_eq!(r.picked_rank, Some(2));
        assert_eq!(r.options[0].champion_id, 2);
        assert_eq!(r.options[1].comfort, 0.5);
        assert!((r.options[1].anyone - 1.0).abs() < 1e-9);
        assert_eq!(r.options[1].base, 1.0);
        assert_eq!(r.options[1].vs, 0.0);
    }

    #[test]
    fn unrankable_pick_has_no_rank() {
        let ev = evaluation(vec![rec(1, 1.0, vec![])], vec![]);
        let r = build_review(&ev, &HashMap::new(), 999, "master_plus", SOURCE_LIVE);
        assert_eq!(r.picked_rank, None);
        assert_eq!(r.options.len(), 1);
    }

    #[test]
    fn record_round_trips_through_json() {
        let ev = evaluation(vec![rec(1, 1.0, vec![contrib("with", 0.2)])], vec![]);
        let r = build_review(&ev, &HashMap::new(), 1, "master_plus", SOURCE_LIVE);
        let back: ReviewRecord = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn decision_board_is_the_last_open_session_before_the_lock() {
        let session = |mid: i64, locked: bool| {
            json!({
                "localPlayerCellId": 0,
                "myTeam": [
                    {"cellId": 0, "assignedPosition": "utility", "championId": 0, "championPickIntent": 0},
                    {"cellId": 1, "assignedPosition": "middle", "championId": mid, "championPickIntent": 0},
                ],
                "theirTeam": [],
                "bans": {"myTeamBans": [], "theirTeamBans": []},
                "actions": [[{"type": "pick", "completed": locked, "championId": if locked { 412 } else { 0 }, "actorCellId": 0}]],
            })
        };
        let snaps = vec![session(0, false), session(11, false), session(11, true), session(12, true)];
        let board = decision_board(&snaps).unwrap();
        assert_eq!(board.my_locked, 0);
        assert_eq!(board.allies.iter().find(|s| s.cell_id == 1).unwrap().champion_id, 11);

        assert!(decision_board(&snaps[..2]).is_none(), "never locked");
        assert!(decision_board(&snaps[2..]).is_none(), "no session before the lock");
        assert!(decision_board(&[]).is_none());
    }
}
