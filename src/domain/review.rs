use serde::{Deserialize, Serialize};

pub const REVIEW_VERSION: u32 = 1;
/// Options kept per review: the top of the ranking plus the champion that was actually picked.
pub const REVIEW_TOP: usize = 12;
pub const SOURCE_LIVE: &str = "live";
pub const SOURCE_REPLAYED: &str = "replayed";
pub const MODE_BLIND: &str = "blind";
pub const MODE_COUNTER: &str = "counter";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewAlly {
    pub champion_id: i32,
    pub position: String,
    /// false = still hovering (tentative) when the player locked in.
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewEnemy {
    pub champion_id: i32,
    pub lane: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewOption {
    pub champion_id: i32,
    /// 1-based position in the full ranking.
    pub rank: usize,
    /// "For you": the score the app shows, including the personal bonus.
    pub score: f64,
    /// Sum of the "you" and "mastery" contributions (already capped).
    pub comfort: f64,
    /// score - comfort: how good the pick is for anyone.
    pub anyone: f64,
    /// Sum of the "vs" contributions (0 in blind mode: the lane opponent was unknown).
    pub vs: f64,
    /// Sum of the "with" contributions.
    pub with: f64,
    pub base: f64,
    pub games: Option<u32>,
    pub wins: Option<u32>,
    pub mastery: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewRecord {
    pub version: u32,
    /// "live" (recorded at lock-in) or "replayed" (rebuilt from saved snapshots).
    pub source: String,
    pub role: String,
    pub tier: String,
    pub my_champ: i32,
    /// "blind" when the lane opponent was not known on the board, else "counter".
    pub mode: String,
    pub allies: Vec<ReviewAlly>,
    pub enemies: Vec<ReviewEnemy>,
    pub bans: Vec<i32>,
    pub options: Vec<ReviewOption>,
    /// Number of candidates in the full ranking.
    pub total: usize,
    /// 1-based rank of the picked champion, None when it could not be ranked.
    pub picked_rank: Option<usize>,
}

impl ReviewRecord {
    pub fn is_replayed(&self) -> bool {
        self.source == SOURCE_REPLAYED
    }
}
