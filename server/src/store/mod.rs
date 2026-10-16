//! SQLite store (spec 003): one writer thread, a small read-only pool.

pub mod auth;
pub mod devices;
pub mod messages;
pub mod webhooks;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use tokio::sync::{Semaphore, oneshot};

pub use rusqlite;

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../../migrations/0001_init.sql")),
    (
        "0002_otp_filters",
        include_str!("../../migrations/0002_otp_filters.sql"),
    ),
];

pub const READERS: usize = 3;

#[derive(Debug)]
pub enum StoreError {
    Db(rusqlite::Error),
    /// The writer thread has stopped.
    Closed,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "database error: {e}"),
            StoreError::Closed => f.write_str("store closed"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

#[derive(Clone)]
pub struct Store {
    writer: mpsc::Sender<Job>,
    readers: Arc<Mutex<Vec<Connection>>>,
    permits: Arc<Semaphore>,
    path: PathBuf,
}

/// PRAGMAs applied to every connection.
pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-2000;",
    )
}

/// Apply pending migrations; each one runs in its own transaction.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<u32> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, (_, sql)) in MIGRATIONS.iter().enumerate() {
        let version = i as u32 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version={version}"))?;
        tx.commit()?;
        tracing::info!(version, "applied migration");
    }
    Ok(MIGRATIONS.len() as u32)
}

pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// Open a read-write connection with PRAGMAs and migrations applied.
pub fn open_rw(path: &Path) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                StoreError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        }
        let mut conn = open_rw(path)?;
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let r = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            configure(&r)?;
            readers.push(r);
        }
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("rustybee-writer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .expect("spawn writer thread");
        Ok(Store {
            writer: tx,
            readers: Arc::new(Mutex::new(readers)),
            permits: Arc::new(Semaphore::new(READERS)),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `f` on the writer connection, outside a transaction.
    pub async fn with_writer<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.writer
            .send(Box::new(move |conn| {
                let _ = tx.send(f(conn));
            }))
            .map_err(|_| StoreError::Closed)?;
        Ok(rx.await.map_err(|_| StoreError::Closed)??)
    }

    /// Run `f` in an immediate write transaction; commit on `Ok`.
    pub async fn write<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Transaction) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        self.with_writer(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let r = f(&tx)?;
            tx.commit()?;
            Ok(r)
        })
        .await
    }

    /// Run `f` on a pooled read-only connection.
    pub async fn read<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| StoreError::Closed)?;
        let pool = self.readers.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .lock()
                .unwrap()
                .pop()
                .expect("permit implies a free reader");
            let r = f(&conn);
            pool.lock().unwrap().push(conn);
            r
        })
        .await
        .map_err(|_| StoreError::Closed)?
        .map_err(StoreError::Db)
    }

    pub async fn checkpoint(&self) -> Result<()> {
        self.with_writer(|c| c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)"))
            .await
    }

    /// `VACUUM INTO` a consistent copy at `dest`.
    pub async fn backup(&self, dest: &Path) -> Result<()> {
        let dest = dest.to_string_lossy().to_string();
        self.with_writer(move |c| c.execute("VACUUM INTO ?1", [dest]).map(|_| ()))
            .await
    }
}

/// Helper for optional single-row queries.
pub fn opt<T>(r: rusqlite::Result<T>) -> rusqlite::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    opt(
        conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
            r.get(0)
        }),
    )
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma<T: rusqlite::types::FromSql>(c: &Connection, p: &str) -> T {
        c.query_row(&format!("PRAGMA {p}"), [], |r| r.get(0))
            .unwrap()
    }

    fn check_pragmas(c: &Connection) {
        assert_eq!(pragma::<String>(c, "journal_mode"), "wal");
        assert_eq!(pragma::<i64>(c, "synchronous"), 1);
        assert_eq!(pragma::<i64>(c, "foreign_keys"), 1);
        assert_eq!(pragma::<i64>(c, "busy_timeout"), 5000);
        assert_eq!(pragma::<i64>(c, "temp_store"), 2);
        assert_eq!(pragma::<i64>(c, "cache_size"), -2000);
    }

    #[tokio::test]
    async fn ac_003_01_migrates_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/rustybee.db");
        let store = Store::open(&path).unwrap();
        let v: u32 = store
            .read(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(v, latest_version());
        drop(store);
        // Second start is a no-op.
        let mut c = Connection::open(&path).unwrap();
        assert_eq!(migrate(&mut c).unwrap(), latest_version());
        let tables: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 10);
    }

    #[tokio::test]
    async fn ac_003_02_pragmas_on_every_connection() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("rustybee.db")).unwrap();
        store
            .with_writer(|c| {
                check_pragmas(c);
                Ok(())
            })
            .await
            .unwrap();
        // Hold every reader at once so each pooled connection is checked.
        let mut tasks = vec![];
        for _ in 0..READERS {
            let s = store.clone();
            tasks.push(tokio::spawn(async move {
                s.read(|c| {
                    check_pragmas(c);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    Ok(())
                })
                .await
                .unwrap()
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
    }

    #[tokio::test]
    async fn ac_003_03_concurrent_writes_serialize() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("rustybee.db")).unwrap();
        let mut tasks = vec![];
        for i in 0..200 {
            let s = store.clone();
            tasks.push(tokio::spawn(async move {
                s.write(move |tx| set_setting(tx, &format!("k{i}"), "v"))
                    .await
            }));
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }
        let n: i64 = store
            .read(|c| c.query_row("SELECT count(*) FROM settings", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 200);
    }

    #[tokio::test]
    async fn ac_003_04_backup_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("rustybee.db")).unwrap();
        store.write(|tx| set_setting(tx, "a", "1")).await.unwrap();
        let dest = dir.path().join("backup.db");
        store.backup(&dest).await.unwrap();
        let c = Connection::open(&dest).unwrap();
        assert_eq!(get_setting(&c, "a").unwrap().as_deref(), Some("1"));
        let v: u32 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, latest_version());
    }
}
