package dev.joowdx.rustybee.data

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import dev.joowdx.rustybee.gateway.OutboxRow
import dev.joowdx.rustybee.gateway.OutboxStore
import dev.joowdx.rustybee.gateway.Redact
import dev.joowdx.rustybee.gateway.SeenStore
import dev.joowdx.rustybee.gateway.SentRow
import dev.joowdx.rustybee.gateway.SentState
import dev.joowdx.rustybee.gateway.SentStore

/** `rustybee.db` version 1 (spec 011). */
class Db(context: Context) : SQLiteOpenHelper(context, "rustybee.db", null, 1), OutboxStore, SentStore, SeenStore {

    override fun onConfigure(db: SQLiteDatabase) {
        db.enableWriteAheadLogging()
    }

    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE outbox (seq INTEGER PRIMARY KEY AUTOINCREMENT, frame_id TEXT NOT NULL UNIQUE, " +
                "type TEXT NOT NULL, json TEXT NOT NULL, created_at INTEGER NOT NULL)",
        )
        db.execSQL(
            "CREATE TABLE sent (msg_id TEXT PRIMARY KEY, to_addr TEXT NOT NULL, state TEXT NOT NULL, " +
                "parts INTEGER NOT NULL, parts_sent INTEGER NOT NULL DEFAULT 0, parts_failed INTEGER NOT NULL DEFAULT 0, " +
                "parts_delivered INTEGER NOT NULL DEFAULT 0, error_code TEXT, error_message TEXT, updated_at INTEGER NOT NULL)",
        )
        db.execSQL("CREATE TABLE seen (frame_id TEXT PRIMARY KEY, at INTEGER NOT NULL)")
        db.execSQL(
            "CREATE TABLE log (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, level TEXT NOT NULL, msg TEXT NOT NULL)",
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) = Unit

    // --- outbox

    override fun insert(frameId: String, type: String, json: String, createdAt: Long): Boolean {
        val cv = ContentValues().apply {
            put("frame_id", frameId); put("type", type); put("json", json); put("created_at", createdAt)
        }
        return writableDatabase.insertWithOnConflict("outbox", null, cv, SQLiteDatabase.CONFLICT_IGNORE) != -1L
    }

    override fun hasInbound(uid: String): Boolean {
        val pattern = "%\"uid\":\"" + uid.filter { it.isLetterOrDigit() } + "\"%"
        readableDatabase.rawQuery("SELECT 1 FROM outbox WHERE type='inbound' AND json LIKE ? LIMIT 1", arrayOf(pattern))
            .use { return it.moveToFirst() }
    }

    override fun pending(): List<OutboxRow> =
        readableDatabase.rawQuery("SELECT seq, frame_id, type, json, created_at FROM outbox ORDER BY seq", null).use { c ->
            buildList { while (c.moveToNext()) add(OutboxRow(c.getLong(0), c.getString(1), c.getString(2), c.getString(3), c.getLong(4))) }
        }

    override fun delete(frameId: String): Boolean =
        writableDatabase.delete("outbox", "frame_id = ?", arrayOf(frameId)) > 0

    override fun count(): Int =
        readableDatabase.rawQuery("SELECT COUNT(*) FROM outbox", null).use { it.moveToFirst(); it.getInt(0) }

    // --- sent

    override fun get(msgId: String): SentRow? =
        readableDatabase.rawQuery(
            "SELECT msg_id, to_addr, state, parts, parts_sent, parts_failed, parts_delivered, error_code, error_message, updated_at " +
                "FROM sent WHERE msg_id = ?",
            arrayOf(msgId),
        ).use { c -> if (c.moveToFirst()) c.toSentRow() else null }

    private fun android.database.Cursor.toSentRow() = SentRow(
        msgId = getString(0), to = getString(1), state = getString(2), parts = getInt(3), partsSent = getInt(4),
        partsFailed = getInt(5), partsDelivered = getInt(6), errorCode = getString(7), errorMessage = getString(8),
        updatedAt = getLong(9),
    )

    private fun SentRow.values() = ContentValues().apply {
        put("msg_id", msgId); put("to_addr", to); put("state", state); put("parts", parts)
        put("parts_sent", partsSent); put("parts_failed", partsFailed); put("parts_delivered", partsDelivered)
        put("error_code", errorCode); put("error_message", errorMessage); put("updated_at", updatedAt)
    }

    override fun insert(row: SentRow) {
        writableDatabase.insertWithOnConflict("sent", null, row.values(), SQLiteDatabase.CONFLICT_IGNORE)
    }

    override fun update(row: SentRow) {
        writableDatabase.update("sent", row.values(), "msg_id = ?", arrayOf(row.msgId))
    }

    override fun acceptedBefore(before: Long): List<SentRow> =
        readableDatabase.rawQuery(
            "SELECT msg_id, to_addr, state, parts, parts_sent, parts_failed, parts_delivered, error_code, error_message, updated_at " +
                "FROM sent WHERE state = ? AND updated_at < ?",
            arrayOf(SentState.ACCEPTED, before.toString()),
        ).use { c -> buildList { while (c.moveToNext()) add(c.toSentRow()) } }

    private val sentLock = Any()

    override fun <T> atomically(block: () -> T): T = synchronized(sentLock) {
        val db = writableDatabase
        db.beginTransaction()
        try {
            block().also { db.setTransactionSuccessful() }
        } finally {
            db.endTransaction()
        }
    }

    fun sendsSince(since: Long): Int =
        readableDatabase.rawQuery(
            "SELECT COUNT(*) FROM sent WHERE updated_at >= ? AND state != ?",
            arrayOf(since.toString(), SentState.ACCEPTED),
        ).use { it.moveToFirst(); it.getInt(0) }

    // --- seen

    override fun seen(frameId: String): Boolean =
        readableDatabase.rawQuery("SELECT 1 FROM seen WHERE frame_id = ?", arrayOf(frameId)).use { it.moveToFirst() }

    override fun markSeen(frameId: String, at: Long) {
        val cv = ContentValues().apply { put("frame_id", frameId); put("at", at) }
        writableDatabase.insertWithOnConflict("seen", null, cv, SQLiteDatabase.CONFLICT_IGNORE)
    }

    fun pruneSeen(before: Long) {
        writableDatabase.delete("seen", "at < ?", arrayOf(before.toString()))
    }

    // --- log (capped at 1000 rows, numbers redacted)

    fun log(level: String, msg: String, at: Long = System.currentTimeMillis()) {
        val db = writableDatabase
        val cv = ContentValues().apply { put("at", at); put("level", level); put("msg", Redact.text(msg)) }
        db.insert("log", null, cv)
        db.execSQL("DELETE FROM log WHERE id <= (SELECT MAX(id) - 1000 FROM log)")
    }

    data class LogEntry(val at: Long, val level: String, val msg: String)

    fun recentLog(limit: Int = 100): List<LogEntry> =
        readableDatabase.rawQuery("SELECT at, level, msg FROM log ORDER BY id DESC LIMIT $limit", null).use { c ->
            buildList { while (c.moveToNext()) add(LogEntry(c.getLong(0), c.getString(1), c.getString(2))) }
        }

    fun clearLog() {
        writableDatabase.delete("log", null, null)
    }

    fun wipe() {
        val db = writableDatabase
        listOf("outbox", "sent", "seen", "log").forEach { db.delete(it, null, null) }
    }
}
