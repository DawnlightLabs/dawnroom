//! SQLite job history. One `jobs` table; a completed image job is an image in the library.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JobRecord {
    pub id: String,
    /// "image" for now; "video" arrives in v0.2.
    pub kind: String,
    /// queued | running | completed | failed | cancelled
    pub status: String,
    pub prompt: String,
    pub params: serde_json::Value,
    pub seed: i64,
    pub model: String,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub output_path: Option<String>,
    pub error: Option<String>,
    /// Frame number: counts every Library job ever submitted, like frames on a roll. Never
    /// reused. 0 for a custom model's hidden test image, which takes no number.
    pub frame: i64,
    pub favourite: bool,
    /// An edit: the Library image it was edited from (the job whose output has the same sha256
    /// as the input image), if it came from the Library.
    #[serde(default)]
    pub parent_id: Option<String>,
}

pub struct Db(Mutex<Connection>);

const COLS: &str = "id, kind, status, prompt, params, seed, model, created_ms, started_ms, finished_ms, output_path, error, COALESCE(frame, rowid), favourite, parent_id";
/// What counts as an image in the library.
const IMAGES: &str = "status = 'completed' AND kind = 'image'";

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS jobs (
                 id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 status TEXT NOT NULL,
                 prompt TEXT NOT NULL,
                 params TEXT NOT NULL,
                 seed INTEGER NOT NULL,
                 model TEXT NOT NULL,
                 created_ms INTEGER NOT NULL,
                 started_ms INTEGER,
                 finished_ms INTEGER,
                 output_path TEXT,
                 error TEXT
             );
             CREATE INDEX IF NOT EXISTS jobs_created ON jobs(created_ms);
             CREATE TABLE IF NOT EXISTS model_usage (
                 model TEXT PRIMARY KEY,
                 last_used_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS licence_acceptances (
                 model TEXT PRIMARY KEY,
                 licence_url TEXT NOT NULL,
                 accepted_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS file_hashes (
                 path TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 mtime_ms INTEGER NOT NULL,
                 sha256 TEXT NOT NULL
             );",
        )?;
        // Columns added after the first schema; older databases gain them here.
        for (col, ddl) in [
            ("favourite", "ALTER TABLE jobs ADD COLUMN favourite INTEGER NOT NULL DEFAULT 0"),
            ("frame", "ALTER TABLE jobs ADD COLUMN frame INTEGER"),
            ("parent_id", "ALTER TABLE jobs ADD COLUMN parent_id TEXT"),
            ("output_sha256", "ALTER TABLE jobs ADD COLUMN output_sha256 TEXT"),
        ] {
            if !conn.prepare("SELECT 1 FROM pragma_table_info('jobs') WHERE name = ?1")?.exists(params![col])? {
                conn.execute_batch(ddl)?;
            }
        }
        conn.execute_batch("UPDATE jobs SET frame = rowid WHERE frame IS NULL; CREATE INDEX IF NOT EXISTS jobs_output_sha ON jobs(output_sha256);")?;
        Ok(Self(Mutex::new(conn)))
    }

    /// Records that a job just loaded this model (the storage page shows "last used").
    pub fn touch_model(&self, model: &str, now_ms: i64) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO model_usage (model, last_used_ms) VALUES (?1, ?2) ON CONFLICT(model) DO UPDATE SET last_used_ms = excluded.last_used_ms",
            params![model, now_ms],
        )?;
        Ok(())
    }

    /// The user accepted a model's licence (by model id, without the size tag: one licence covers
    /// every tag). Accepting again just updates the time.
    pub fn accept_licence(&self, model: &str, licence_url: &str, now_ms: i64) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO licence_acceptances (model, licence_url, accepted_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(model) DO UPDATE SET licence_url = excluded.licence_url, accepted_ms = excluded.accepted_ms",
            params![model, licence_url, now_ms],
        )?;
        Ok(())
    }

    /// When the user accepted this licence for this model; `None` if never, or if they accepted
    /// a different licence URL (the terms changed, so it's asked again).
    pub fn licence_accepted(&self, model: &str, licence_url: &str) -> Option<i64> {
        let conn = self.0.lock().unwrap();
        conn.query_row("SELECT accepted_ms FROM licence_acceptances WHERE model = ?1 AND licence_url = ?2", params![model, licence_url], |r| r.get(0)).ok()
    }

    /// When each model was last used, by id.
    pub fn last_used(&self) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
        let conn = self.0.lock().unwrap();
        let mut st = conn.prepare("SELECT model, last_used_ms FROM model_usage")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        rows.collect()
    }

    /// Points every stored image path outside `outputs` at the same file name inside it. Image
    /// paths are stored whole, so after the data folder moves they'd point at the old drive.
    /// Returns how many rows changed.
    pub fn rebase_outputs(&self, outputs: &Path) -> rusqlite::Result<usize> {
        let conn = self.0.lock().unwrap();
        let rows: Vec<(String, String)> = {
            let mut st = conn.prepare("SELECT id, output_path FROM jobs WHERE output_path IS NOT NULL")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let mut n = 0;
        for (id, old) in rows {
            let old_path = Path::new(&old);
            if old_path.starts_with(outputs) {
                continue;
            }
            let Some(name) = old_path.file_name() else { continue };
            let new = outputs.join(name);
            n += conn.execute("UPDATE jobs SET output_path = ?1 WHERE id = ?2", params![new.to_string_lossy(), id])?;
        }
        Ok(n)
    }

    /// Folds the write-ahead log into the main file, so copying the folder copies a whole database.
    pub fn checkpoint(&self) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
    }

    /// Jobs left queued/running by a previous engine that died can never finish.
    pub fn fail_stale(&self, now_ms: i64) -> rusqlite::Result<usize> {
        self.0.lock().unwrap().execute(
            "UPDATE jobs SET status = 'failed', finished_ms = ?1,
                 error = 'The engine stopped before this job finished. Run it again.'
             WHERE status IN ('queued', 'running')",
            params![now_ms],
        )
    }

    /// Saves a new job and returns its frame number (`j.frame` is ignored: the next free one is
    /// used). Test jobs (a custom model's hidden test image) get frame 0 and use up no number.
    pub fn insert(&self, j: &JobRecord) -> rusqlite::Result<i64> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "INSERT INTO jobs (id, kind, status, prompt, params, seed, model, created_ms, started_ms, finished_ms, output_path, error, favourite, parent_id, frame)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     CASE WHEN ?2 = 'test' THEN 0 ELSE (SELECT COALESCE(MAX(COALESCE(frame, rowid)), 0) + 1 FROM jobs) END)",
            params![j.id, j.kind, j.status, j.prompt, j.params.to_string(), j.seed, j.model, j.created_ms, j.started_ms, j.finished_ms, j.output_path, j.error, j.favourite, j.parent_id],
        )?;
        conn.query_row("SELECT frame FROM jobs WHERE id = ?1", params![j.id], |r| r.get(0))
    }

    /// Returns false when there's no completed image with that id.
    pub fn set_favourite(&self, id: &str, favourite: bool) -> rusqlite::Result<bool> {
        let n = self.0.lock().unwrap().execute(&format!("UPDATE jobs SET favourite = ?2 WHERE id = ?1 AND {IMAGES}"), params![id, favourite])?;
        Ok(n > 0)
    }

    /// Takes an image out of the library. The row stays (status `deleted`) so its frame number
    /// is never handed out again; the caller removes the file.
    pub fn mark_deleted(&self, id: &str) -> rusqlite::Result<bool> {
        let n = self.0.lock().unwrap().execute(&format!("UPDATE jobs SET status = 'deleted', output_path = NULL, favourite = 0 WHERE id = ?1 AND {IMAGES}"), params![id])?;
        Ok(n > 0)
    }

    pub fn mark_running(&self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute("UPDATE jobs SET status = 'running', started_ms = ?2 WHERE id = ?1", params![id, now_ms])?;
        Ok(())
    }

    /// `output_sha256` is the saved file's hash: an edit whose input has it links back here.
    pub fn mark_completed(&self, id: &str, now_ms: i64, output_path: &str, output_sha256: &str) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE jobs SET status = 'completed', finished_ms = ?2, output_path = ?3, output_sha256 = ?4 WHERE id = ?1",
            params![id, now_ms, output_path, output_sha256],
        )?;
        Ok(())
    }

    /// The Library image whose file has this sha256 (the newest, if the same bytes were saved twice).
    pub fn image_by_sha256(&self, sha256: &str) -> rusqlite::Result<Option<String>> {
        self.0
            .lock()
            .unwrap()
            .query_row(&format!("SELECT id FROM jobs WHERE output_sha256 = ?1 AND {IMAGES} ORDER BY created_ms DESC LIMIT 1"), params![sha256], |r| r.get(0))
            .optional()
    }

    pub fn mark_failed(&self, id: &str, now_ms: i64, error: &str) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE jobs SET status = 'failed', finished_ms = ?2, error = ?3 WHERE id = ?1",
            params![id, now_ms, error],
        )?;
        Ok(())
    }

    pub fn mark_cancelled(&self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE jobs SET status = 'cancelled', finished_ms = ?2, error = 'The job was cancelled.' WHERE id = ?1",
            params![id, now_ms],
        )?;
        Ok(())
    }

    /// A cached sha256 for a model file, valid only while its size and mtime are unchanged.
    pub fn cached_hash(&self, path: &str, size: u64, mtime_ms: i64) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT sha256 FROM file_hashes WHERE path = ?1 AND size = ?2 AND mtime_ms = ?3",
                params![path, size as i64, mtime_ms],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn save_hash(&self, path: &str, size: u64, mtime_ms: i64, sha256: &str) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT OR REPLACE INTO file_hashes (path, size, mtime_ms, sha256) VALUES (?1, ?2, ?3, ?4)",
            params![path, size as i64, mtime_ms, sha256],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<JobRecord>> {
        self.0.lock().unwrap().query_row(&format!("SELECT {COLS} FROM jobs WHERE id = ?1"), params![id], from_row).optional()
    }

    /// Completed images, newest first, plus how many match. `q` finds text in the prompt
    /// (case-insensitive); `favourites` keeps only favourites.
    pub fn list_images(&self, limit: u32, offset: u32, q: Option<&str>, favourites: bool) -> rusqlite::Result<(Vec<JobRecord>, i64)> {
        let conn = self.0.lock().unwrap();
        // LIKE, with the wildcards in the user's text escaped by `!`.
        let like = q.map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{}%", q.replace('!', "!!").replace('%', "!%").replace('_', "!_")));
        let filter = format!("{IMAGES} AND (?1 IS NULL OR prompt LIKE ?1 ESCAPE '!') AND (?2 = 0 OR favourite = 1)");
        let total = conn.query_row(&format!("SELECT COUNT(*) FROM jobs WHERE {filter}"), params![like, favourites], |r| r.get(0))?;
        let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM jobs WHERE {filter} ORDER BY created_ms DESC, id LIMIT ?3 OFFSET ?4"))?;
        let items = stmt.query_map(params![like, favourites, limit, offset], from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }
}

fn from_row(r: &Row) -> rusqlite::Result<JobRecord> {
    let params: String = r.get(4)?;
    Ok(JobRecord {
        id: r.get(0)?,
        kind: r.get(1)?,
        status: r.get(2)?,
        prompt: r.get(3)?,
        params: serde_json::from_str(&params).unwrap_or(serde_json::Value::Null),
        seed: r.get(5)?,
        model: r.get(6)?,
        created_ms: r.get(7)?,
        started_ms: r.get(8)?,
        finished_ms: r.get(9)?,
        output_path: r.get(10)?,
        error: r.get(11)?,
        frame: r.get(12)?,
        favourite: r.get(13)?,
        parent_id: r.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn image_paths_follow_a_moved_folder() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        conn.execute("INSERT INTO jobs (id, kind, status, prompt, params, seed, model, created_ms, output_path) VALUES ('a','image','completed','p','{}',1,'m',0,'D:/old/outputs/a.png')", []).unwrap();
        drop(conn);
        let new = std::path::Path::new("C:/new/outputs");
        assert_eq!(db.rebase_outputs(new).unwrap(), 1);
        assert_eq!(db.rebase_outputs(new).unwrap(), 0, "already inside");
        assert_eq!(db.get("a").unwrap().unwrap().output_path.unwrap(), new.join("a.png").to_string_lossy());
    }

    use super::*;

    fn job(id: &str, prompt: &str, created_ms: i64) -> JobRecord {
        JobRecord {
            id: id.into(),
            kind: "image".into(),
            status: "completed".into(),
            prompt: prompt.into(),
            params: serde_json::json!({}),
            seed: 1,
            model: "m".into(),
            created_ms,
            started_ms: None,
            finished_ms: None,
            output_path: Some(format!("{id}.png")),
            error: None,
            frame: 0,
            favourite: false,
            parent_id: None,
        }
    }

    #[test]
    fn test_jobs_take_no_frame_number() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.insert(&job("a", "p", 1)).unwrap(), 1);
        let test = JobRecord { kind: "test".into(), ..job("t", "apple", 2) };
        assert_eq!(db.insert(&test).unwrap(), 0);
        assert_eq!(db.insert(&job("b", "p", 3)).unwrap(), 2, "the next image gets the next number");
    }

    #[test]
    fn edits_find_their_source_by_output_hash() {
        let db = Db::open_in_memory().unwrap();
        db.insert(&JobRecord { status: "queued".into(), output_path: None, ..job("a", "p", 1) }).unwrap();
        db.mark_completed("a", 2, "a.png", &"ab".repeat(32)).unwrap();
        assert_eq!(db.image_by_sha256(&"ab".repeat(32)).unwrap().as_deref(), Some("a"));
        assert_eq!(db.image_by_sha256(&"cd".repeat(32)).unwrap(), None);
        db.insert(&JobRecord { parent_id: Some("a".into()), ..job("b", "make it blue", 3) }).unwrap();
        assert_eq!(db.get("b").unwrap().unwrap().parent_id.as_deref(), Some("a"));
        assert!(db.mark_deleted("a").unwrap());
        assert_eq!(db.image_by_sha256(&"ab".repeat(32)).unwrap(), None, "deleted images aren't sources");
    }

    #[test]
    fn licence_acceptance_is_per_model_and_per_licence_url() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.licence_accepted("flux1-dev", "https://a"), None);
        db.accept_licence("flux1-dev", "https://a", 5).unwrap();
        assert_eq!(db.licence_accepted("flux1-dev", "https://a"), Some(5));
        assert_eq!(db.licence_accepted("flux1-dev", "https://b"), None, "new terms are asked again");
        assert_eq!(db.licence_accepted("flux1-schnell", "https://a"), None);
    }

    #[test]
    fn frames_count_up_and_survive_deletes() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.insert(&job("a", "a red door", 1)).unwrap(), 1);
        assert_eq!(db.insert(&job("b", "a blue door", 2)).unwrap(), 2);
        assert!(db.mark_deleted("b").unwrap());
        assert!(!db.mark_deleted("b").unwrap(), "already gone");
        assert_eq!(db.insert(&job("c", "100% wool", 3)).unwrap(), 3, "a deleted frame's number isn't reused");
        assert_eq!(db.get("c").unwrap().unwrap().frame, 3);
        let (items, total) = db.list_images(50, 0, None, false).unwrap();
        assert_eq!((total, items.iter().map(|j| j.id.as_str()).collect::<Vec<_>>()), (2, vec!["c", "a"]));
    }

    #[test]
    fn search_and_favourites_filter_the_library() {
        let db = Db::open_in_memory().unwrap();
        for (id, prompt) in [("a", "a red door"), ("b", "A Blue DOOR"), ("c", "100% wool")] {
            db.insert(&job(id, prompt, 1)).unwrap();
        }
        assert_eq!(db.list_images(50, 0, Some("door"), false).unwrap().1, 2);
        assert_eq!(db.list_images(50, 0, Some("  "), false).unwrap().1, 3, "blank search is no search");
        assert_eq!(db.list_images(50, 0, Some("%"), false).unwrap().1, 1, "% is literal");
        assert_eq!(db.list_images(50, 0, Some("_"), false).unwrap().1, 0, "_ is literal");
        assert!(db.set_favourite("b", true).unwrap());
        assert!(!db.set_favourite("nope", true).unwrap());
        let (items, total) = db.list_images(50, 0, None, true).unwrap();
        assert_eq!((total, items[0].id.as_str(), items[0].favourite), (1, "b", true));
        assert_eq!(db.list_images(50, 0, Some("red"), true).unwrap().1, 0);
    }

    #[test]
    fn older_databases_gain_the_new_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE jobs (id TEXT PRIMARY KEY, kind TEXT NOT NULL, status TEXT NOT NULL, prompt TEXT NOT NULL, params TEXT NOT NULL,
                 seed INTEGER NOT NULL, model TEXT NOT NULL, created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, output_path TEXT, error TEXT);
             INSERT INTO jobs VALUES ('old', 'image', 'completed', 'p', '{}', 1, 'm', 1, NULL, NULL, 'old.png', NULL);",
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        let old = db.get("old").unwrap().unwrap();
        assert_eq!((old.frame, old.favourite), (1, false));
        assert_eq!(db.insert(&job("new", "p", 2)).unwrap(), 2);
    }
}
