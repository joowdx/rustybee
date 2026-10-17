//! Periodic maintenance: TTL sweep, status timeout, pruning, WAL checkpoint.

use crate::app::{App, UiEvent};
use crate::store::messages::{self, MessageRow};
use crate::store::{self};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DAY_MS: i64 = 24 * 3600 * 1000;

pub async fn run(app: App) {
    let mut last_checkpoint = Instant::now();
    let mut last_prune: Option<i64> = None;
    loop {
        tokio::time::sleep(app.cfg.sweep_interval).await;
        if let Err(e) = sweep_once(&app).await {
            tracing::warn!(error = %e, "sweep");
        }
        if let Err(e) = crate::otp::resolve(&app).await {
            tracing::warn!(error = %e, "login code sweep");
        }
        let now = app.clock.now_ms();
        if last_prune.is_none_or(|t| now - t >= DAY_MS) {
            last_prune = Some(now);
            if let Err(e) = prune_once(&app).await {
                tracing::warn!(error = %e, "prune");
            }
            match backup_once(&app).await {
                Ok(Some(p)) => tracing::info!(path = %p.display(), "backup written"),
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "backup"),
            }
        }
        if last_checkpoint.elapsed() >= Duration::from_secs(3600) {
            last_checkpoint = Instant::now();
            if let Err(e) = app.store.checkpoint().await {
                tracing::warn!(error = %e, "checkpoint");
            }
        }
    }
}

/// The daily prune: seen frames, auth rows and, when `RUSTYBEE_RETENTION_DAYS`
/// is set, finished messages and webhook deliveries (spec 003). Returns the
/// number of messages and deliveries deleted.
pub async fn prune_once(app: &App) -> store::Result<(usize, usize)> {
    let now = app.clock.now_ms();
    let retention = app.cfg.retention_days.map(|d| now - i64::from(d) * DAY_MS);
    let (msgs, hooks) = app
        .store
        .write(move |c| {
            c.execute(
                "DELETE FROM seen_frames WHERE seen_at<?1",
                [now - 7 * DAY_MS],
            )?;
            store::auth::prune(c, now)?;
            store::idempotency::prune(c, now)?;
            let Some(before) = retention else {
                return Ok((0, 0));
            };
            // Deliveries first; any left pointing at a pruned message get NULL.
            let hooks = store::webhooks::prune_finished(c, before)?;
            let msgs = messages::prune_finished(c, before)?;
            Ok((msgs, hooks))
        })
        .await?;
    if msgs + hooks > 0 {
        tracing::info!(messages = msgs, deliveries = hooks, "retention pruned");
    }
    Ok((msgs, hooks))
}

const BACKUP_PREFIX: &str = "rustybee-";
const BACKUP_SUFFIX: &str = ".db";

/// `rustybee-YYYY-MM-DD.db` for the UTC day of `now_ms`.
pub fn backup_name(now_ms: i64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp(now_ms.div_euclid(1000))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{BACKUP_PREFIX}{:04}-{:02}-{:02}{BACKUP_SUFFIX}",
        t.year(),
        u8::from(t.month()),
        t.day()
    )
}

fn is_backup_name(name: &str) -> bool {
    name.strip_prefix(BACKUP_PREFIX)
        .and_then(|r| r.strip_suffix(BACKUP_SUFFIX))
        .is_some_and(|d| {
            d.len() == 10
                && d.bytes().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                })
        })
}

/// Delete all but the newest `keep` dated backups in `dir`. Returns how
/// many were deleted. Other files are left alone.
fn rotate(dir: &Path, keep: usize) -> std::io::Result<usize> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| is_backup_name(n))
        .collect();
    names.sort();
    let old = names.len().saturating_sub(keep);
    for n in &names[..old] {
        std::fs::remove_file(dir.join(n))?;
    }
    Ok(old)
}

/// The daily backup (spec 003): when `RUSTYBEE_BACKUP_DIR` is set, write
/// today's `VACUUM INTO` copy (0600) through a temporary file, check it with
/// `PRAGMA integrity_check`, then keep the newest `RUSTYBEE_BACKUP_KEEP`.
/// Today's backup is not rewritten if it already exists.
pub async fn backup_once(app: &App) -> Result<Option<PathBuf>, String> {
    let Some(dir) = app.cfg.backup_dir.clone() else {
        return Ok(None);
    };
    let keep = app.cfg.backup_keep.max(1) as usize;
    let dest = dir.join(backup_name(app.clock.now_ms()));
    let tmp = dest.with_extension("db.tmp");
    let shown = dir.display().to_string();
    let prepared = {
        let (dir, dest, tmp) = (dir.clone(), dest.clone(), tmp.clone());
        tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
            store::private_dir(&dir)?;
            if dest.exists() {
                return Ok(false);
            }
            match std::fs::remove_file(&tmp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(true),
            }
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{shown}: {e}"))?
    };
    if prepared {
        app.store.backup(&tmp).await.map_err(|e| e.to_string())?;
        let (dest, tmp) = (dest.clone(), tmp.clone());
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let checked = store::check_backup(&tmp).map_err(|e| e.to_string())?;
            if let Err(problem) = checked {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("integrity check failed: {problem}"));
            }
            std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    let removed = tokio::task::spawn_blocking(move || rotate(&dir, keep))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if removed > 0 {
        tracing::info!(removed, "old backups rotated");
    }
    Ok(prepared.then_some(dest))
}

/// One TTL / status-timeout pass (spec 004). Returns the changed messages.
pub async fn sweep_once(app: &App) -> store::Result<Vec<MessageRow>> {
    let now = app.clock.now_ms();
    let ttl = app.cfg.message_ttl.as_millis() as i64;
    let timeout = app.cfg.status_timeout.as_millis() as i64;
    let changed = app
        .store
        .write(move |c| messages::sweep(c, now, ttl, timeout))
        .await?;
    if !changed.is_empty() {
        tracing::info!(count = changed.len(), "sweeper changed messages");
        app.webhook_notify.notify_one();
        for m in &changed {
            app.hub.notify(m.device_id);
            app.publish(UiEvent::Message(m.public_id.clone()));
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_003_10_backup_names() {
        assert_eq!(backup_name(0), "rustybee-1970-01-01.db");
        assert_eq!(backup_name(1_760_000_000_000), "rustybee-2025-10-09.db");
        assert!(is_backup_name("rustybee-2026-10-17.db"));
        for n in [
            "rustybee.db",
            "rustybee-2026-10-17.db.tmp",
            "rustybee-2026-1-17.db",
            "other-2026-10-17.db",
            "rustybee-2026x10-17.db",
        ] {
            assert!(!is_backup_name(n), "{n}");
        }
    }
}
