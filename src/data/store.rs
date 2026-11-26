use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::config::data_dir;
use crate::domain::review::ReviewRecord;

pub const DRAFT_GAME_WINDOW: f64 = 15.0 * 60.0;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS drafts (
    id INTEGER PRIMARY KEY,
    started REAL NOT NULL,
    ended REAL,
    role TEXT,
    tier TEXT,
    my_champ INTEGER,
    game_id INTEGER,
    result TEXT
);
CREATE TABLE IF NOT EXISTS snapshots (
    draft_id INTEGER NOT NULL REFERENCES drafts(id),
    t REAL NOT NULL,
    session BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS recommendations (
    draft_id INTEGER NOT NULL REFERENCES drafts(id),
    t REAL NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS games (
    game_id INTEGER PRIMARY KEY,
    created REAL NOT NULL,
    queue INTEGER,
    champ INTEGER,
    role TEXT,
    win INTEGER,
    duration INTEGER,
    detail TEXT
);
CREATE INDEX IF NOT EXISTS games_champ ON games(champ, role);
"#;

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn pack_json(val: &Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(val).unwrap_or_default();
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = enc.write_all(&bytes);
    enc.finish().unwrap_or_default()
}

pub fn unpack_json(blob: &[u8]) -> Option<Value> {
    let mut dec = ZlibDecoder::new(blob);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).ok()?;
    serde_json::from_slice(&out).ok()
}

/// One recorded draft in which the player picked a champion.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftRow {
    pub id: i64,
    pub started: f64,
    pub role: Option<String>,
    pub tier: Option<String>,
    pub my_champ: i32,
    pub result: Option<String>,
    pub game_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredReview {
    pub draft: DraftRow,
    /// Latest "review" record saved for the draft, if any.
    pub review: Option<ReviewRecord>,
}

const DRAFT_COLS: &str = "id, started, role, tier, my_champ, result, game_id";

fn draft_row(r: &rusqlite::Row) -> rusqlite::Result<DraftRow> {
    Ok(DraftRow {
        id: r.get(0)?,
        started: r.get(1)?,
        role: r.get(2)?,
        tier: r.get(3)?,
        my_champ: r.get(4)?,
        result: r.get(5)?,
        game_id: r.get(6)?,
    })
}

pub struct Store {
    db: Mutex<Connection>,
    last_hash: Mutex<HashMap<i64, [u8; 20]>>,
    /// The logged-in League account (puuid). New drafts and games are stamped with it, and
    /// reads only return that account's rows. `None` (never seen a client) reads everything.
    account: Mutex<Option<String>>,
}

/// Rows belonging to the current account; `?1` is the account (NULL matches all rows).
const MINE: &str = "(?1 IS NULL OR puuid = ?1)";

fn has_column(conn: &Connection, table: &str, col: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for n in names {
        if n? == col {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Adds the `puuid` account column to stores created before accounts were tracked.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    for table in ["drafts", "games"] {
        if !has_column(conn, table, "puuid")? {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN puuid TEXT;"))?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS drafts_puuid ON drafts(puuid, started);
         CREATE INDEX IF NOT EXISTS games_puuid ON games(puuid);",
    )
}

impl Store {
    pub fn open_default() -> Result<Self, rusqlite::Error> {
        Self::open(data_dir().join("store.sqlite"))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let p: PathBuf = path.as_ref().to_path_buf();
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(p)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self {
            db: Mutex::new(conn),
            last_hash: Mutex::new(HashMap::new()),
            account: Mutex::new(None),
        })
    }

    pub fn account(&self) -> Option<String> {
        self.account.lock().unwrap().clone()
    }

    /// Switches the store to `puuid`. The first account ever seen also claims the rows
    /// recorded before accounts were tracked; later accounts leave them alone.
    pub fn set_account(&self, puuid: &str) -> Result<(), rusqlite::Error> {
        if puuid.is_empty() {
            return Ok(());
        }
        {
            let db = self.db.lock().unwrap();
            let owned: i64 = db.query_row(
                "SELECT (SELECT COUNT(*) FROM drafts WHERE puuid IS NOT NULL) \
                      + (SELECT COUNT(*) FROM games WHERE puuid IS NOT NULL)",
                [],
                |r| r.get(0),
            )?;
            if owned == 0 {
                db.execute("UPDATE drafts SET puuid=?1 WHERE puuid IS NULL", params![puuid])?;
                db.execute("UPDATE games SET puuid=?1 WHERE puuid IS NULL", params![puuid])?;
            }
        }
        *self.account.lock().unwrap() = Some(puuid.to_string());
        Ok(())
    }

    pub fn start_draft(&self, role: Option<&str>, tier: &str) -> Result<i64, rusqlite::Error> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO drafts(started, role, tier, puuid) VALUES (?,?,?,?)",
            params![now_secs(), role, tier, acct],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn update_draft(
        &self,
        draft_id: i64,
        ended: Option<f64>,
        my_champ: Option<i32>,
        role: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let db = self.db.lock().unwrap();
        if let Some(e) = ended {
            db.execute("UPDATE drafts SET ended=? WHERE id=?", params![e, draft_id])?;
        }
        if let Some(c) = my_champ {
            db.execute("UPDATE drafts SET my_champ=? WHERE id=?", params![c, draft_id])?;
        }
        if let Some(r) = role {
            db.execute("UPDATE drafts SET role=? WHERE id=?", params![r, draft_id])?;
        }
        Ok(())
    }

    /// Dedup by hashing serialized JSON *before* zlib compression to avoid wasted CPU on repeats.
    pub fn snapshot(&self, draft_id: i64, session: &Value) -> Result<(), rusqlite::Error> {
        let raw = serde_json::to_vec(session).unwrap_or_default();
        let mut hasher = Sha1::new();
        hasher.update(&raw);
        let digest: [u8; 20] = hasher.finalize().into();
        {
            let mut lh = self.last_hash.lock().unwrap();
            if lh.get(&draft_id) == Some(&digest) {
                return Ok(());
            }
            lh.insert(draft_id, digest);
        }
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        let _ = enc.write_all(&raw);
        let blob = enc.finish().unwrap_or_default();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO snapshots VALUES (?,?,?)",
            params![draft_id, now_secs(), blob],
        )?;
        Ok(())
    }

    pub fn save_recs(&self, draft_id: i64, kind: &str, payload: &Value) -> Result<(), rusqlite::Error> {
        let s = serde_json::to_string(payload).unwrap_or_default();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO recommendations VALUES (?,?,?,?)",
            params![draft_id, now_secs(), kind, s],
        )?;
        Ok(())
    }

    pub fn has_game(&self, game_id: i64) -> Result<bool, rusqlite::Error> {
        let db = self.db.lock().unwrap();
        let row: Option<i64> = db
            .query_row("SELECT 1 FROM games WHERE game_id=?", params![game_id], |r| r.get(0))
            .optional()?;
        Ok(row.is_some())
    }

    pub fn add_game(
        &self,
        game_id: i64,
        created: f64,
        queue: i64,
        champ: i32,
        role: &str,
        win: bool,
        duration: i64,
        detail: &Value,
    ) -> Result<(), rusqlite::Error> {
        let s = serde_json::to_string(detail).unwrap_or_default();
        let acct = self.account();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR IGNORE INTO games(game_id, created, queue, champ, role, win, duration, detail, puuid) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            params![game_id, created, queue, champ, role, i32::from(win), duration, s, acct],
        )?;
        Ok(())
    }

    pub fn draft_role_for(&self, champ: i32, created: f64) -> Option<String> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        let res: Option<Option<String>> = db
            .query_row(
                &format!(
                    "SELECT role FROM drafts WHERE {MINE} AND my_champ=?2 AND ended IS NOT NULL \
                     AND ended BETWEEN ?3 AND ?4 ORDER BY ABS(?5 - ended) LIMIT 1"
                ),
                params![acct, champ, created - DRAFT_GAME_WINDOW, created + 60.0, created],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        res.flatten()
    }

    pub fn link_draft(&self, game_id: i64, champ: i32, created: f64, win: bool) -> Result<(), rusqlite::Error> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        let id: Option<i64> = db
            .query_row(
                &format!(
                    "SELECT id FROM drafts WHERE {MINE} AND game_id IS NULL AND my_champ=?2 \
                     AND ended IS NOT NULL AND ended BETWEEN ?3 AND ?4 ORDER BY ABS(?5 - ended) LIMIT 1"
                ),
                params![acct, champ, created - DRAFT_GAME_WINDOW, created + 60.0, created],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            let result = if win { "win" } else { "loss" };
            db.execute(
                "UPDATE drafts SET game_id=?, result=? WHERE id=?",
                params![game_id, result, id],
            )?;
        }
        Ok(())
    }

    /// Most recent drafts (newest first) that have a picked champion, each with its latest review.
    pub fn recent_reviews(&self, limit: usize) -> Vec<StoredReview> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        let Ok(mut stmt) = db.prepare(&format!(
            "SELECT {DRAFT_COLS} FROM drafts WHERE {MINE} AND my_champ IS NOT NULL AND my_champ != 0 \
             ORDER BY started DESC, id DESC LIMIT ?2"
        )) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![acct, limit as i64], draft_row) else {
            return Vec::new();
        };
        let drafts: Vec<DraftRow> = rows.filter_map(|r| r.ok()).collect();
        drafts
            .into_iter()
            .map(|draft| {
                let payload: Option<String> = db
                    .query_row(
                        "SELECT payload FROM recommendations WHERE draft_id=? AND kind='review'                          ORDER BY t DESC, rowid DESC LIMIT 1",
                        params![draft.id],
                        |r| r.get(0),
                    )
                    .optional()
                    .ok()
                    .flatten();
                let review = payload.and_then(|p| serde_json::from_str(&p).ok());
                StoredReview { draft, review }
            })
            .collect()
    }

    /// Among the `recent` newest drafts with a picked champion, those with no "review" row yet.
    pub fn drafts_lacking_review(&self, recent: usize) -> Vec<DraftRow> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        let Ok(mut stmt) = db.prepare(&format!(
            "SELECT {DRAFT_COLS} FROM (SELECT {DRAFT_COLS} FROM drafts \
             WHERE {MINE} AND my_champ IS NOT NULL AND my_champ != 0 ORDER BY started DESC, id DESC LIMIT ?2) \
             WHERE id NOT IN (SELECT draft_id FROM recommendations WHERE kind='review') \
             ORDER BY started DESC, id DESC"
        )) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![acct, recent as i64], draft_row) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Raw LCU sessions saved for a draft, oldest first.
    pub fn snapshots(&self, draft_id: i64) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let Ok(mut stmt) =
            db.prepare("SELECT session FROM snapshots WHERE draft_id=? ORDER BY t, rowid")
        else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![draft_id], |r| r.get::<_, Vec<u8>>(0)) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).filter_map(|b| unpack_json(&b)).collect()
    }

    pub fn top_roles(&self, limit: usize) -> Vec<String> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        let Ok(mut stmt) = db.prepare(&format!(
            "SELECT role FROM games WHERE {MINE} AND role IS NOT NULL AND role != '' \
             GROUP BY role ORDER BY COUNT(*) DESC LIMIT ?2"
        )) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![acct, limit as i64], |r| r.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Returns champion_id -> (games, wins, eff_games, eff_wins)
    pub fn champ_record(
        &self,
        role: Option<&str>,
        half_life_days: f64,
    ) -> HashMap<i32, (u32, u32, f64, f64)> {
        let acct = self.account();
        let db = self.db.lock().unwrap();
        let mut out: HashMap<i32, (u32, u32, f64, f64)> = HashMap::new();
        let now = now_secs();
        let decay = if half_life_days > 0.0 {
            2.0_f64.ln() / (half_life_days * 86400.0)
        } else {
            0.0
        };

        let mut push_row = |cid: i32, win: i32, created: f64| {
            let w = if decay > 0.0 {
                (-(now - created).max(0.0) * decay).exp()
            } else {
                1.0
            };
            let e = out.entry(cid).or_insert((0, 0, 0.0, 0.0));
            e.0 += 1;
            if win != 0 {
                e.1 += 1;
            }
            e.2 += w;
            if win != 0 {
                e.3 += w;
            }
        };

        // `?2` NULL means every role.
        if let Ok(mut stmt) = db.prepare(&format!(
            "SELECT champ, win, created FROM games WHERE {MINE} AND (?2 IS NULL OR role = ?2)"
        )) && let Ok(rows) = stmt.query_map(params![acct, role], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?, row.get::<_, f64>(2)?))
        }) {
            for row in rows.flatten() {
                push_row(row.0, row.1, row.2);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_store_draft_and_games() {
        let dir = std::env::temp_dir().join("draftly_test_store");
        let _ = fs::remove_dir_all(&dir);
        let store = Store::open(dir.join("s.sqlite")).unwrap();
        let did = store.start_draft(Some("support"), "master_plus").unwrap();
        store.snapshot(did, &json!({"a": 1})).unwrap();
        store.snapshot(did, &json!({"a": 1})).unwrap();
        store.update_draft(did, Some(1000.0), Some(412), Some("support")).unwrap();
        store.add_game(99, 1050.0, 420, 412, "support", true, 1800, &json!({})).unwrap();
        store.link_draft(99, 412, 1050.0, true).unwrap();
        assert!(store.has_game(99).unwrap());
        assert_eq!(store.top_roles(2), vec!["support"]);
        let rec = store.champ_record(Some("support"), 90.0);
        assert_eq!(rec[&412].0, 1);
        assert_eq!(rec[&412].1, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    fn review(champ: i32, total: usize) -> ReviewRecord {
        ReviewRecord {
            version: 1,
            source: "live".into(),
            role: "support".into(),
            tier: "master_plus".into(),
            my_champ: champ,
            mode: "blind".into(),
            allies: vec![],
            enemies: vec![],
            bans: vec![],
            options: vec![],
            total,
            picked_rank: Some(1),
        }
    }

    #[test]
    fn test_recent_reviews_order_and_joins() {
        let dir = std::env::temp_dir().join("draftly_test_store_reviews");
        let _ = fs::remove_dir_all(&dir);
        let store = Store::open(dir.join("s.sqlite")).unwrap();

        let a = store.start_draft(Some("support"), "master_plus").unwrap();
        store.update_draft(a, Some(1000.0), Some(412), Some("support")).unwrap();
        store.save_recs(a, "review", &serde_json::to_value(review(412, 10)).unwrap()).unwrap();
        store.save_recs(a, "review", &serde_json::to_value(review(412, 20)).unwrap()).unwrap();
        store.add_game(99, 1050.0, 420, 412, "support", false, 1800, &json!({})).unwrap();
        store.link_draft(99, 412, 1050.0, false).unwrap();

        let b = store.start_draft(Some("middle"), "master_plus").unwrap();
        store.update_draft(b, None, Some(103), Some("middle")).unwrap();
        store.save_recs(b, "locked", &json!({"role": "middle"})).unwrap();

        // never locked in: not listed
        let c = store.start_draft(Some("top"), "master_plus").unwrap();

        let all = store.recent_reviews(10);
        let ids: Vec<i64> = all.iter().map(|r| r.draft.id).collect();
        assert_eq!(ids, vec![b, a]);
        assert!(!ids.contains(&c));
        assert!(all[0].review.is_none());
        assert_eq!(all[0].draft.result, None);
        let first = &all[1];
        assert_eq!(first.draft.result.as_deref(), Some("loss"));
        assert_eq!(first.draft.game_id, Some(99));
        assert_eq!(first.draft.my_champ, 412);
        assert_eq!(first.review.as_ref().unwrap().total, 20, "latest review row wins");
        assert_eq!(store.recent_reviews(1).len(), 1);

        let lacking: Vec<i64> = store.drafts_lacking_review(10).iter().map(|d| d.id).collect();
        assert_eq!(lacking, vec![b]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A locked-in draft for `champ` that ended at `ended`.
    fn locked_draft(store: &Store, champ: i32, ended: f64) -> i64 {
        let d = store.start_draft(Some("support"), "master_plus").unwrap();
        store.update_draft(d, Some(ended), Some(champ), Some("support")).unwrap();
        d
    }

    #[test]
    fn test_history_is_scoped_to_the_logged_in_account() {
        let dir = std::env::temp_dir().join("draftly_test_store_accounts");
        let _ = fs::remove_dir_all(&dir);
        let store = Store::open(dir.join("s.sqlite")).unwrap();

        // Recorded before accounts were tracked: the first account seen claims it.
        let legacy = locked_draft(&store, 1, 100.0);
        store.add_game(10, 150.0, 420, 1, "support", true, 1800, &json!({})).unwrap();
        store.set_account("main").unwrap();
        let main_draft = locked_draft(&store, 2, 200.0);

        // A second account claims nothing and sees only its own rows.
        store.set_account("alt").unwrap();
        let alt_draft = locked_draft(&store, 3, 300.0);
        store.add_game(11, 350.0, 420, 3, "middle", false, 1800, &json!({})).unwrap();
        store.link_draft(11, 3, 350.0, false).unwrap();
        let ids = |s: &Store| s.recent_reviews(10).iter().map(|r| r.draft.id).collect::<Vec<_>>();
        assert_eq!(ids(&store), vec![alt_draft]);
        assert_eq!(store.drafts_lacking_review(10).len(), 1);
        assert_eq!(store.top_roles(5), vec!["middle"]);
        assert_eq!(store.champ_record(None, 0.0).keys().copied().collect::<Vec<_>>(), vec![3]);
        assert_eq!(store.draft_role_for(1, 150.0), None, "main's draft is not the alt's");

        // Switching back restores the main's history, including the claimed legacy rows.
        store.set_account("main").unwrap();
        assert_eq!(ids(&store), vec![main_draft, legacy]);
        assert_eq!(store.top_roles(5), vec!["support"]);
        assert!(store.champ_record(None, 0.0).contains_key(&1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_old_store_gains_the_account_column() {
        let dir = std::env::temp_dir().join("draftly_test_store_migrate");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sqlite");
        {
            // The pre-account schema, with one locked draft and one game.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(
                "INSERT INTO drafts(started, ended, role, tier, my_champ) VALUES (1, 2, 'support', 'master_plus', 7);
                 INSERT INTO games VALUES (5, 3, 420, 7, 'support', 1, 1800, '{}');",
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.recent_reviews(10).len(), 1, "no account yet: everything is visible");
        store.set_account("main").unwrap();
        assert_eq!(store.recent_reviews(10).len(), 1);
        assert_eq!(store.top_roles(5), vec!["support"]);
        drop(store);
        // Reopening an already-migrated store is a no-op.
        let store = Store::open(&path).unwrap();
        store.set_account("main").unwrap();
        assert_eq!(store.recent_reviews(10).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshots_come_back_in_order() {
        let dir = std::env::temp_dir().join("draftly_test_store_snaps");
        let _ = fs::remove_dir_all(&dir);
        let store = Store::open(dir.join("s.sqlite")).unwrap();
        let d = store.start_draft(Some("support"), "master_plus").unwrap();
        for n in 0..3 {
            store.snapshot(d, &json!({"n": n})).unwrap();
        }
        let snaps = store.snapshots(d);
        let ns: Vec<i64> = snaps.iter().map(|s| s["n"].as_i64().unwrap()).collect();
        assert_eq!(ns, vec![0, 1, 2]);
        let _ = fs::remove_dir_all(&dir);
    }
}
