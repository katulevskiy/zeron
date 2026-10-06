use crate::{Error, Result, now};
use axum::body::Bytes;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::sync::{mpsc, oneshot};

pub struct Db {
    pub connection: Connection,
    pub snapshot: Option<(Bytes, String)>,
}
impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS claims (item TEXT NOT NULL, task TEXT NOT NULL, actor TEXT NOT NULL,
                revision TEXT NOT NULL, expires TEXT NOT NULL, PRIMARY KEY(item,task));
            CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, item TEXT NOT NULL, actor TEXT NOT NULL,
                action TEXT NOT NULL, at TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS deliveries (id TEXT PRIMARY KEY, at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS outbox (item TEXT PRIMARY KEY, attempts INTEGER NOT NULL DEFAULT 0,
                error TEXT, retry_at TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS events_item_time ON events(item,at DESC);
            CREATE INDEX IF NOT EXISTS claims_expiry ON claims(expires);")?;
        Ok(Self {
            connection,
            snapshot: None,
        })
    }
    pub fn transaction<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        match f(self) {
            Ok(result) => {
                if let Err(e) = self.connection.execute_batch("COMMIT") {
                    let _ = self.connection.execute_batch("ROLLBACK");
                    return Err(e.into());
                }
                self.snapshot = None;
                Ok(result)
            }
            Err(error) => {
                self.connection.execute_batch("ROLLBACK")?;
                Err(error)
            }
        }
    }
    pub fn get(&self, id: &str) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .connection
            .prepare_cached("SELECT data FROM items WHERE id=?")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn list(&self) -> Result<Vec<Value>> {
        let mut stmt = self
            .connection
            .prepare_cached("SELECT data FROM items ORDER BY rowid")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .map(|s| Ok(serde_json::from_str(&s?)?))
            .collect()
    }
    pub fn put(&self, item: &Value) -> Result<()> {
        self.connection
            .prepare_cached(
                "INSERT INTO items VALUES (?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            )?
            .execute(params![crate::s(item, "id"), item.to_string()])?;
        Ok(())
    }
    pub fn all_claims(&self) -> Result<Vec<Value>> {
        let mut stmt = self.connection.prepare_cached(
            "SELECT item,task,actor,revision,expires FROM claims WHERE expires>?",
        )?;
        Ok(stmt.query_map([now()], |r| Ok(json!({"item":r.get::<_,String>(0)?,"task":r.get::<_,String>(1)?,"actor":r.get::<_,String>(2)?,"revision":r.get::<_,String>(3)?,"expires":r.get::<_,String>(4)?})))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }
    pub fn claims(&self, id: &str) -> Result<Vec<Value>> {
        let mut stmt = self.connection.prepare_cached(
            "SELECT item,task,actor,revision,expires FROM claims WHERE item=? AND expires>?",
        )?;
        Ok(stmt.query_map(params![id,now()], |r| Ok(json!({"item":r.get::<_,String>(0)?,"task":r.get::<_,String>(1)?,"actor":r.get::<_,String>(2)?,"revision":r.get::<_,String>(3)?,"expires":r.get::<_,String>(4)?})))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }
    pub fn claim(
        &self,
        id: &str,
        task: &str,
        actor: &str,
        revision: &str,
        expires: &str,
    ) -> Result<()> {
        self.connection.prepare_cached("INSERT INTO claims VALUES (?,?,?,?,?) ON CONFLICT(item,task) DO UPDATE SET actor=excluded.actor,revision=excluded.revision,expires=excluded.expires")?.execute(params![id,task,actor,revision,expires])?;
        Ok(())
    }
    pub fn release(&self, id: &str, task: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM claims WHERE item=? AND task=?",
            params![id, task],
        )?;
        Ok(())
    }
    pub fn clear_claims(&self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM claims WHERE item=?", [id])?;
        Ok(())
    }
    pub fn event(&self, id: &str, actor: &str, action: &str, data: &Value) -> Result<()> {
        self.connection
            .prepare_cached("INSERT INTO events VALUES (?,?,?,?,?,?)")?
            .execute(params![
                uuid::Uuid::new_v4().to_string(),
                id,
                actor,
                action,
                now(),
                data.to_string()
            ])?;
        Ok(())
    }
    pub fn events(&self, id: &str) -> Result<Vec<Value>> {
        let mut stmt = self.connection.prepare_cached("SELECT id,item,actor,action,at,data FROM events WHERE item=? ORDER BY at DESC,rowid DESC LIMIT 60")?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| { let (id,item,actor,action,at,data)=row?; Ok(json!({"id":id,"item":item,"actor":actor,"action":action,"at":at,"data":serde_json::from_str::<Value>(&data)?})) }).collect()
    }
    pub fn value(&self, key: &str) -> Result<Value> {
        let raw: Option<String> = self
            .connection
            .prepare_cached("SELECT data FROM kv WHERE key=?")?
            .query_row([key], |r| r.get(0))
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
            .map(|v| v.unwrap_or(Value::Null))
    }
    pub fn set(&self, key: &str, value: &Value) -> Result<()> {
        self.connection
            .prepare_cached(
                "INSERT INTO kv VALUES (?,?) ON CONFLICT(key) DO UPDATE SET data=excluded.data",
            )?
            .execute(params![key, value.to_string()])?;
        Ok(())
    }
    pub fn remove(&self, key: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM kv WHERE key=?", [key])?;
        Ok(())
    }
    pub fn enqueue(&self, id: &str) -> Result<()> {
        self.connection.execute("INSERT INTO outbox(item,retry_at) VALUES (?,?) ON CONFLICT(item) DO UPDATE SET attempts=0,error=NULL,retry_at=excluded.retry_at",params![id,now()])?;
        Ok(())
    }
    pub fn pending(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .connection
            .prepare_cached("SELECT item FROM outbox WHERE retry_at<=?")?;
        Ok(stmt
            .query_map([now()], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn sent(&self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM outbox WHERE item=?", [id])?;
        Ok(())
    }
    pub fn failed(&self, id: &str, message: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE outbox SET attempts=attempts+1,error=?,retry_at=? WHERE item=?",
            params![
                message.chars().take(300).collect::<String>(),
                (chrono::Utc::now() + chrono::Duration::minutes(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                id
            ],
        )?;
        Ok(())
    }
    pub fn expire_claims(&self) -> Result<()> {
        let expired: Vec<(String, String, String)> = {
            let mut stmt = self
                .connection
                .prepare("DELETE FROM claims WHERE expires<=? RETURNING item,task,actor")?;
            stmt.query_map([now()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, task, actor) in expired {
            self.event(&id, &actor, "claim-expired", &json!({"task":task}))?;
            self.enqueue(&id)?;
        }
        Ok(())
    }
    pub fn delivered(&self, id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM deliveries WHERE id=?)",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn mark_delivered(&self, id: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR IGNORE INTO deliveries VALUES (?,?)",
            params![id, now()],
        )?;
        Ok(())
    }
}

type Job = Box<dyn FnOnce(&mut Db) + Send>;
#[derive(Clone)]
pub struct Store {
    sender: mpsc::Sender<Job>,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Db::open(path)?;
        let (sender, mut receiver) = mpsc::channel::<Job>(256);
        std::thread::Builder::new()
            .name("review-sqlite".into())
            .spawn(move || {
                while let Some(job) = receiver.blocking_recv() {
                    job(&mut db);
                }
            })?;
        Ok(Self { sender })
    }
    pub async fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Db) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Box::new(move |db| {
                let _ = sender.send(job(db));
            }))
            .await
            .map_err(|_| Error::internal("Database worker stopped"))?;
        receiver
            .await
            .map_err(|_| Error::internal("Database worker stopped"))?
    }
    pub async fn write<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Db) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.read(move |db| db.transaction(job)).await
    }
}
