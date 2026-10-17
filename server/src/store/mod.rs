//! SQLite store (spec 003): one writer thread, a small read-only pool.

pub mod auth;
pub mod devices;
pub mod idempotency;
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
    (
        "0003_fcm_wake",
        include_str!("../../migrations/0003_fcm_wake.sql"),
    ),
    (
        "0004_idempotency_indexes",
        include_str!("../../migrations/0004_idempotency_indexes.sql"),
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

fn other_error(msg: String) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(msg)))
}

/// Apply pending migrations; each one runs in its own transaction. A database
/// from a newer build (a higher `user_version`) is refused, never touched.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<u32> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let latest = latest_version();
    if current > latest {
        return Err(other_error(format!(
            "the database is at schema version {current}, but this rustybee build only knows \
             version {latest}; run a newer rustybee or restore a backup"
        )));
    }
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

/// Owner-only permissions (unix): 0700 for directories, 0600 for files.
#[cfg(unix)]
mod private {
    use std::fs::{self, DirBuilder, OpenOptions, Permissions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub fn create_dir(dir: &Path) -> io::Result<()> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }

    /// Create `path` empty with 0600 if it is missing; tighten it if not.
    pub fn file(path: &Path) -> io::Result<()> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => restrict(path),
            Err(e) => Err(e),
        }
    }

    /// Set 0600 on an existing file; a missing one is fine.
    pub fn restrict(path: &Path) -> io::Result<()> {
        match fs::set_permissions(path, Permissions::from_mode(0o600)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

#[cfg(not(unix))]
mod private {
    use std::io;
    use std::path::Path;

    pub fn create_dir(dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)
    }

    pub fn file(_: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn restrict(_: &Path) -> io::Result<()> {
        Ok(())
    }
}

fn io_error(e: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

/// `<db>-wal` and `<db>-shm`.
fn sidecars(path: &Path) -> [PathBuf; 2] {
    ["-wal", "-shm"].map(|ext| {
        let mut p = path.as_os_str().to_owned();
        p.push(ext);
        PathBuf::from(p)
    })
}

/// Create a directory (and missing parents) 0700 on unix.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    private::create_dir(dir)
}

/// Make sure a file SQLite is about to create is private (0600 on unix).
/// Used for the database itself and for backups before `VACUUM INTO`.
pub fn private_file(path: &Path) -> rusqlite::Result<()> {
    private::file(path).map_err(io_error)
}

/// Open a read-write connection with PRAGMAs and migrations applied. The
/// data directory is created 0700 and the database, WAL and SHM files are
/// kept 0600 (unix).
pub fn open_rw(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        private::create_dir(dir).map_err(io_error)?;
    }
    private_file(path)?;
    for p in sidecars(path) {
        private::restrict(&p).map_err(io_error)?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&mut conn)?;
    // SQLite gives new WAL/SHM files the database's mode; this covers older files.
    for p in sidecars(path) {
        private::restrict(&p).map_err(io_error)?;
    }
    Ok(conn)
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
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

    /// `VACUUM INTO` a consistent copy at `dest`, created 0600 (unix). Runs
    /// on a read-only pooled connection, so writes are not held up.
    pub async fn backup(&self, dest: &Path) -> Result<()> {
        private_file(dest)?;
        let dest = dest.to_string_lossy().to_string();
        self.read(move |c| vacuum_into(c, &dest)).await
    }
}

fn vacuum_into(c: &Connection, dest: &str) -> rusqlite::Result<()> {
    c.execute("VACUUM INTO ?1", [dest]).map(|_| ())
}

/// `rustybee backup`: open the database read-only (no migrations, no
/// writes) and `VACUUM INTO` a 0600 copy at `dest`.
pub fn backup_file(src: &Path, dest: &Path) -> rusqlite::Result<()> {
    let c = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    private_file(dest)?;
    vacuum_into(&c, &dest.to_string_lossy())
}

/// `PRAGMA integrity_check` on a backup, opened read-only.
pub fn check_backup(path: &Path) -> rusqlite::Result<std::result::Result<(), String>> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let r: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    Ok(if r == "ok" { Ok(()) } else { Err(r) })
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
        assert_eq!(tables, 11);
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

    #[test]
    fn ac_003_07_refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rustybee.db");
        let c = Connection::open(&path).unwrap();
        let newer = latest_version() + 5;
        c.execute_batch(&format!("PRAGMA user_version={newer}"))
            .unwrap();
        drop(c);
        let e = Store::open(&path).err().expect("refused").to_string();
        assert!(e.contains(&format!("version {newer}")), "{e}");
        assert!(e.contains(&format!("version {}", latest_version())), "{e}");
        // Nothing was changed.
        let c = Connection::open(&path).unwrap();
        assert_eq!(pragma::<u32>(&c, "user_version"), newer);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ac_003_08_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let path = data.join("rustybee.db");
        let store = Store::open(&path).unwrap();
        store.write(|tx| set_setting(tx, "a", "1")).await.unwrap();
        assert_eq!(mode(&data), 0o700);
        assert_eq!(mode(&path), 0o600);
        for p in sidecars(&path) {
            assert_eq!(mode(&p), 0o600, "{}", p.display());
        }
        let dest = data.join("backup.db");
        store.backup(&dest).await.unwrap();
        assert_eq!(mode(&dest), 0o600);
        drop(store);
        // Files left world-readable by an older build are tightened on open.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _store = Store::open(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn ac_003_09_backup_command_opens_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        // A database from before any migration: open_rw would migrate it.
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(42);")
            .unwrap();
        drop(c);
        let dest = dir.path().join("copy.db");
        backup_file(&path, &dest).unwrap();
        let c = Connection::open(&path).unwrap();
        assert_eq!(pragma::<u32>(&c, "user_version"), 0, "source was migrated");
        let copy = Connection::open(&dest).unwrap();
        let x: i64 = copy.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 42);
        assert_eq!(check_backup(&dest).unwrap(), Ok(()));
        // A WAL database with no server running (no -wal/-shm left behind).
        let wal = dir.path().join("wal.db");
        let c = open_rw(&wal).unwrap();
        set_setting(&c, "a", "1").unwrap();
        drop(c);
        assert!(!sidecars(&wal)[0].exists());
        let dest = dir.path().join("wal-copy.db");
        backup_file(&wal, &dest).unwrap();
        let copy = Connection::open(&dest).unwrap();
        assert_eq!(get_setting(&copy, "a").unwrap().as_deref(), Some("1"));
        // A missing database is an error, not a new empty file.
        assert!(backup_file(&dir.path().join("none.db"), &dir.path().join("x.db")).is_err());
        assert!(!dir.path().join("none.db").exists());
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
