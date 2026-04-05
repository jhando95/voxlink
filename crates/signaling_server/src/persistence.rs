use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// Token lifetime: 90 days in seconds
const TOKEN_LIFETIME_SECS: i64 = 90 * 86400;

/// Persists spaces, channels, and messages to SQLite.
/// Only cold data is stored — runtime state (peers, rooms) stays in-memory.
pub struct Database {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct SpaceRow {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub owner_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ChannelRow {
    pub id: String,
    pub space_id: String,
    pub name: String,
    pub room_key: String,
    pub channel_type: String, // "voice" or "text"
    pub topic: Option<String>,
    pub voice_quality: Option<u8>, // 0=Low, 1=Standard, 2=High, 3=Ultra
    pub min_role: Option<String>,  // "member", "moderator", "admin", "owner"
    pub position: Option<u32>,
    pub auto_delete_hours: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: String,
    pub channel_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: i64,
    pub edited: bool,
    pub reply_to_message_id: Option<String>,
    pub reply_to_sender_name: Option<String>,
    pub reply_preview: Option<String>,
    pub pinned: bool,
    pub link_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub user_id: String,
    pub token: String,
    pub display_name: String,
    pub created_at: i64,
    pub issued_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct BanRow {
    pub space_id: String,
    pub user_id: String,
    pub banned_at: i64,
}

#[derive(Debug, Clone)]
pub struct SpaceRoleRow {
    pub space_id: String,
    pub user_id: String,
    pub role: String,
    pub assigned_at: i64,
    /// Hex color for this role (e.g. "#ff5555"), empty for default
    pub role_color: String,
}

#[derive(Debug, Clone)]
pub struct AuditLogRow {
    pub id: String,
    pub space_id: String,
    pub actor_user_id: String,
    pub actor_name: String,
    pub action: String,
    pub target_user_id: Option<String>,
    pub target_name: Option<String>,
    pub detail: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct FriendRequestRow {
    pub requester_id: String,
    pub addressee_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct FriendshipRow {
    pub user_low_id: String,
    pub user_high_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct DirectMessageRow {
    pub id: String,
    pub user_low_id: String,
    pub user_high_id: String,
    pub sender_user_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: i64,
    pub edited: bool,
    pub reply_to_message_id: Option<String>,
    pub reply_to_sender_name: Option<String>,
    pub reply_preview: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GroupMessageRow {
    pub id: String,
    pub group_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: i64,
    pub edited: bool,
    pub reply_to_message_id: Option<String>,
    pub reply_to_sender_name: Option<String>,
    pub reply_preview: Option<String>,
}

impl Database {
    /// Lock the DB connection, recovering from poisoned mutex (a prior panic in a
    /// spawn_blocking task). This prevents a single DB error from crashing all future ops.
    pub fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        match self.conn.lock() {
            Ok(guard) => Ok(guard),
            Err(poisoned) => Ok(poisoned.into_inner()),
        }
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open DB: {e}"))?;

        // WAL mode for concurrent reads during audio relay
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("Failed to set WAL mode: {e}"))?;

        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<(), String> {
        {
            let conn = self.lock_conn()?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS spaces (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    invite_code TEXT NOT NULL UNIQUE,
                    owner_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS channels (
                    id TEXT PRIMARY KEY,
                    space_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    room_key TEXT NOT NULL,
                    channel_type TEXT NOT NULL DEFAULT 'voice',
                    FOREIGN KEY (space_id) REFERENCES spaces(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY,
                    channel_id TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    sender_name TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    edited INTEGER NOT NULL DEFAULT 0,
                    reply_to_message_id TEXT,
                    reply_to_sender_name TEXT,
                    reply_preview TEXT,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS users (
                    user_id TEXT PRIMARY KEY,
                    token TEXT NOT NULL UNIQUE,
                    display_name TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    issued_at INTEGER NOT NULL DEFAULT 0,
                    last_seen_at INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS bans (
                    space_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    banned_at INTEGER NOT NULL,
                    PRIMARY KEY (space_id, user_id),
                    FOREIGN KEY (space_id) REFERENCES spaces(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS space_roles (
                    space_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    assigned_at INTEGER NOT NULL,
                    PRIMARY KEY (space_id, user_id),
                    FOREIGN KEY (space_id) REFERENCES spaces(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS space_audit_log (
                    id TEXT PRIMARY KEY,
                    space_id TEXT NOT NULL,
                    actor_user_id TEXT NOT NULL,
                    actor_name TEXT NOT NULL,
                    action TEXT NOT NULL,
                    target_user_id TEXT,
                    target_name TEXT,
                    detail TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    FOREIGN KEY (space_id) REFERENCES spaces(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS friend_requests (
                    requester_id TEXT NOT NULL,
                    addressee_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (requester_id, addressee_id)
                );
                CREATE TABLE IF NOT EXISTS friendships (
                    user_low_id TEXT NOT NULL,
                    user_high_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (user_low_id, user_high_id)
                );
                CREATE TABLE IF NOT EXISTS direct_messages (
                    id TEXT PRIMARY KEY,
                    user_low_id TEXT NOT NULL,
                    user_high_id TEXT NOT NULL,
                    sender_user_id TEXT NOT NULL,
                    sender_name TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    edited INTEGER NOT NULL DEFAULT 0,
                    reply_to_message_id TEXT,
                    reply_to_sender_name TEXT,
                    reply_preview TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_channels_space ON channels(space_id);
                CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id);
                CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
                CREATE INDEX IF NOT EXISTS idx_users_token ON users(token);
                CREATE INDEX IF NOT EXISTS idx_bans_space ON bans(space_id);
                CREATE INDEX IF NOT EXISTS idx_space_roles_space ON space_roles(space_id);
                CREATE INDEX IF NOT EXISTS idx_space_audit_log_space ON space_audit_log(space_id, created_at);
                CREATE INDEX IF NOT EXISTS idx_friend_requests_addressee ON friend_requests(addressee_id);
                CREATE INDEX IF NOT EXISTS idx_friendships_low ON friendships(user_low_id);
                CREATE INDEX IF NOT EXISTS idx_friendships_high ON friendships(user_high_id);
                CREATE INDEX IF NOT EXISTS idx_direct_messages_pair ON direct_messages(user_low_id, user_high_id);
                CREATE INDEX IF NOT EXISTS idx_direct_messages_timestamp ON direct_messages(timestamp);

                CREATE TABLE IF NOT EXISTS user_blocks (
                    blocker_id TEXT NOT NULL,
                    blocked_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (blocker_id, blocked_id)
                );

                CREATE TABLE IF NOT EXISTS group_conversations (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS group_members (
                    group_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    PRIMARY KEY (group_id, user_id)
                );
                CREATE TABLE IF NOT EXISTS group_messages (
                    id TEXT PRIMARY KEY,
                    group_id TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    sender_name TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    edited INTEGER NOT NULL DEFAULT 0,
                    reply_to_message_id TEXT,
                    reply_to_sender_name TEXT,
                    reply_preview TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_group_messages_group ON group_messages(group_id, timestamp);
                CREATE INDEX IF NOT EXISTS idx_group_members_group ON group_members(group_id);

                CREATE TABLE IF NOT EXISTS space_nicknames (
                    space_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    nickname TEXT NOT NULL,
                    PRIMARY KEY (space_id, user_id)
                );

                CREATE TABLE IF NOT EXISTS automod_filters (
                    space_id TEXT NOT NULL,
                    word TEXT NOT NULL,
                    action TEXT NOT NULL DEFAULT 'block',
                    UNIQUE(space_id, word)
                );
                CREATE INDEX IF NOT EXISTS idx_automod_filters_space ON automod_filters(space_id);

                CREATE TABLE IF NOT EXISTS scheduled_events (
                    id TEXT PRIMARY KEY,
                    space_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '',
                    start_time INTEGER NOT NULL,
                    end_time INTEGER NOT NULL DEFAULT 0,
                    creator_id TEXT NOT NULL,
                    creator_name TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_events_space ON scheduled_events(space_id);

                CREATE TABLE IF NOT EXISTS event_interests (
                    event_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    PRIMARY KEY (event_id, user_id)
                );

                CREATE TABLE IF NOT EXISTS scheduled_messages (
                    id TEXT PRIMARY KEY,
                    space_id TEXT NOT NULL,
                    channel_id TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    sender_name TEXT NOT NULL,
                    content TEXT NOT NULL,
                    send_at INTEGER NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_sched_msg_send ON scheduled_messages(send_at);",
            )
            .map_err(|e| format!("Failed to init tables: {e}"))?;
        }
        self.ensure_message_column("edited", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_message_column("reply_to_message_id", "TEXT")?;
        self.ensure_message_column("reply_to_sender_name", "TEXT")?;
        self.ensure_message_column("reply_preview", "TEXT")?;
        self.ensure_message_column("pinned", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column("channels", "topic", "TEXT NOT NULL DEFAULT ''")?;
        self.ensure_column("users", "status", "TEXT NOT NULL DEFAULT ''")?;
        self.ensure_column("users", "issued_at", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column("users", "last_seen_at", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column("channels", "voice_quality", "INTEGER NOT NULL DEFAULT 2")?;
        self.ensure_column("channels", "min_role", "TEXT NOT NULL DEFAULT 'member'")?;
        self.ensure_column("channels", "position", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column("users", "bio", "TEXT NOT NULL DEFAULT ''")?;
        // v0.8.0 migration columns
        self.ensure_column("spaces", "invite_expires_at", "INTEGER")?;
        self.ensure_column("spaces", "invite_max_uses", "INTEGER")?;
        self.ensure_column("spaces", "invite_uses", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_message_column("forwarded_from", "TEXT")?;
        // v0.10.0: Role colors
        self.ensure_column("space_roles", "role_color", "TEXT NOT NULL DEFAULT ''")?;
        // Auto-delete & link preview columns
        self.ensure_column("channels", "auto_delete_hours", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_message_column("link_url", "TEXT")?;
        // Welcome message
        self.ensure_column("spaces", "welcome_message", "TEXT NOT NULL DEFAULT ''")?;
        // Account system columns
        self.ensure_column("users", "email", "TEXT")?;
        self.ensure_column("users", "password_hash", "TEXT")?;
        // Server discovery
        self.ensure_column("spaces", "is_public", "INTEGER DEFAULT 0")?;
        // Index on email for login lookups
        {
            let conn = self.lock_conn()?;
            let _ = conn.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users(email) WHERE email IS NOT NULL",
                [],
            );
        }
        Ok(())
    }

    fn ensure_message_column(&self, column: &str, definition: &str) -> Result<(), String> {
        self.ensure_column("messages", column, definition)
    }

    fn ensure_column(&self, table: &str, column: &str, definition: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
        match conn.execute(&sql, []) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_ERROR =>
            {
                Ok(())
            }
            Err(e) => Err(format!("Failed to migrate {table} table: {e}")),
        }
    }

    // ─── Spaces ───

    pub fn load_all_spaces(&self) -> Result<Vec<SpaceRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT id, name, invite_code, owner_id, created_at FROM spaces")
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SpaceRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    invite_code: row.get(2)?,
                    owner_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn save_space(&self, space: &SpaceRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO spaces (id, name, invite_code, owner_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![space.id, space.name, space.invite_code, space.owner_id, space.created_at],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn delete_space(&self, space_id: &str) -> Result<(), String> {
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin delete transaction: {e}"))?;
        tx.execute(
            "DELETE FROM messages WHERE channel_id IN (SELECT id FROM channels WHERE space_id = ?1)",
            params![space_id],
        )
        .map_err(|e| format!("Delete messages error: {e}"))?;
        tx.execute(
            "DELETE FROM channels WHERE space_id = ?1",
            params![space_id],
        )
        .map_err(|e| format!("Delete channels error: {e}"))?;
        tx.execute("DELETE FROM bans WHERE space_id = ?1", params![space_id])
            .map_err(|e| format!("Delete bans error: {e}"))?;
        tx.execute(
            "DELETE FROM space_roles WHERE space_id = ?1",
            params![space_id],
        )
        .map_err(|e| format!("Delete roles error: {e}"))?;
        tx.execute(
            "DELETE FROM space_audit_log WHERE space_id = ?1",
            params![space_id],
        )
        .map_err(|e| format!("Delete audit log error: {e}"))?;
        tx.execute("DELETE FROM spaces WHERE id = ?1", params![space_id])
            .map_err(|e| format!("Delete space error: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit delete transaction: {e}"))?;
        Ok(())
    }

    pub fn rename_space(&self, space_id: &str, name: &str) {
        if let Ok(conn) = self.lock_conn() {
            let _ = conn.execute(
                "UPDATE spaces SET name = ?1 WHERE id = ?2",
                params![name, space_id],
            );
        }
    }

    pub fn set_space_description(&self, space_id: &str, description: &str) {
        if let Ok(conn) = self.lock_conn() {
            let _ = self.ensure_column("spaces", "description", "TEXT DEFAULT ''");
            let _ = conn.execute(
                "UPDATE spaces SET description = ?1 WHERE id = ?2",
                params![description, space_id],
            );
        }
    }

    // ─── Channels ───

    pub fn load_channels_for_space(&self, space_id: &str) -> Result<Vec<ChannelRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT id, space_id, name, room_key, channel_type, topic, voice_quality, min_role, position, auto_delete_hours FROM channels WHERE space_id = ?1 ORDER BY position ASC")
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![space_id], |row| {
                Ok(ChannelRow {
                    id: row.get(0)?,
                    space_id: row.get(1)?,
                    name: row.get(2)?,
                    room_key: row.get(3)?,
                    channel_type: row.get(4)?,
                    topic: row.get(5)?,
                    voice_quality: row.get::<_, Option<u8>>(6).ok().flatten(),
                    min_role: row.get(7)?,
                    position: row.get::<_, Option<u32>>(8).ok().flatten(),
                    auto_delete_hours: row.get::<_, Option<u32>>(9).ok().flatten(),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn save_channel(&self, ch: &ChannelRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO channels (id, space_id, name, room_key, channel_type, topic, voice_quality, min_role, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                ch.id,
                ch.space_id,
                ch.name,
                ch.room_key,
                ch.channel_type,
                ch.topic.as_deref().unwrap_or(""),
                ch.voice_quality.unwrap_or(2),
                ch.min_role.as_deref().unwrap_or("member"),
                ch.position.unwrap_or(0)
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn set_channel_topic(&self, channel_id: &str, topic: &str) {
        if let Ok(conn) = self.lock_conn() {
            let _ = conn.execute(
                "UPDATE channels SET topic = ?1 WHERE id = ?2",
                params![topic, channel_id],
            );
        }
    }

    pub fn set_channel_auto_delete(&self, channel_id: &str, hours: u32) {
        if let Ok(conn) = self.lock_conn() {
            let _ = conn.execute(
                "UPDATE channels SET auto_delete_hours = ?1 WHERE id = ?2",
                params![hours, channel_id],
            );
        }
    }

    /// Delete messages older than the auto-delete threshold for all channels that have it enabled.
    /// Returns the total number of deleted messages.
    pub fn delete_expired_messages(&self) -> Result<usize, String> {
        let conn = self.lock_conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        // Find channels with auto_delete_hours > 0
        let mut stmt = conn
            .prepare("SELECT id, auto_delete_hours FROM channels WHERE auto_delete_hours > 0")
            .map_err(|e| format!("Query error: {e}"))?;
        let channels: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Query error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        let mut total_deleted: usize = 0;
        for (channel_id, hours) in channels {
            let cutoff = now - hours * 3600;
            let deleted = conn
                .execute(
                    "DELETE FROM messages WHERE channel_id = ?1 AND timestamp < ?2",
                    params![channel_id, cutoff],
                )
                .unwrap_or(0);
            total_deleted += deleted;
        }
        Ok(total_deleted)
    }

    /// Load the auto_delete_hours for a channel (0 = disabled).
    pub fn get_channel_auto_delete(&self, channel_id: &str) -> u32 {
        let Ok(conn) = self.lock_conn() else {
            return 0;
        };
        conn.query_row(
            "SELECT auto_delete_hours FROM channels WHERE id = ?1",
            params![channel_id],
            |row| row.get::<_, u32>(0),
        )
        .unwrap_or(0)
    }

    pub fn set_user_status(&self, user_id: &str, status: &str) {
        if let Ok(conn) = self.lock_conn() {
            let _ = conn.execute(
                "UPDATE users SET status = ?1 WHERE user_id = ?2",
                params![status, user_id],
            );
        }
    }

    pub fn set_user_bio(&self, user_id: &str, bio: &str) {
        if let Ok(conn) = self.lock_conn() {
            let _ = conn.execute(
                "UPDATE users SET bio = ?1 WHERE user_id = ?2",
                params![bio, user_id],
            );
        }
    }

    pub fn get_user_bio(&self, user_id: &str) -> String {
        let Ok(conn) = self.lock_conn() else {
            return String::new();
        };
        conn.query_row(
            "SELECT bio FROM users WHERE user_id = ?1",
            params![user_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
    }

    pub fn delete_channel(&self, channel_id: &str) -> Result<(), String> {
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin channel delete transaction: {e}"))?;
        tx.execute(
            "DELETE FROM messages WHERE channel_id = ?1",
            params![channel_id],
        )
        .map_err(|e| format!("Delete channel messages error: {e}"))?;
        tx.execute("DELETE FROM channels WHERE id = ?1", params![channel_id])
            .map_err(|e| format!("Delete channel error: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit channel delete transaction: {e}"))?;
        Ok(())
    }

    // ─── Messages ───

    pub fn load_messages_for_channel(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, channel_id, sender_id, sender_name, content, timestamp, edited,
                        reply_to_message_id, reply_to_sender_name, reply_preview, pinned, link_url
                 FROM messages WHERE channel_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![channel_id, limit as i64], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    sender_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    content: row.get(4)?,
                    timestamp: row.get(5)?,
                    edited: row.get::<_, i64>(6)? != 0,
                    reply_to_message_id: row.get(7)?,
                    reply_to_sender_name: row.get(8)?,
                    reply_preview: row.get(9)?,
                    pinned: row.get::<_, i64>(10)? != 0,
                    link_url: row.get(11)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        let mut msgs: Vec<_> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))?;
        msgs.reverse(); // Return in chronological order
        Ok(msgs)
    }

    pub fn save_message(&self, msg: &MessageRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO messages (
                id, channel_id, sender_id, sender_name, content, timestamp, edited,
                reply_to_message_id, reply_to_sender_name, reply_preview, pinned, link_url
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                msg.id,
                msg.channel_id,
                msg.sender_id,
                msg.sender_name,
                msg.content,
                msg.timestamp,
                msg.edited as i64,
                msg.reply_to_message_id,
                msg.reply_to_sender_name,
                msg.reply_preview,
                msg.pinned as i64,
                msg.link_url
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn update_message(&self, message_id: &str, new_content: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE messages SET content = ?2, edited = 1 WHERE id = ?1",
                params![message_id, new_content],
            )
            .map_err(|e| format!("Update error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn set_message_pinned(&self, message_id: &str, pinned: bool) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE messages SET pinned = ?2 WHERE id = ?1",
                params![message_id, pinned as i64],
            )
            .map_err(|e| format!("Update error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn delete_message(&self, message_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute("DELETE FROM messages WHERE id = ?1", params![message_id])
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn search_messages(
        &self,
        channel_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<MessageRow>, String> {
        if query.len() > 256 {
            return Err("Search query too long (max 256 characters)".to_string());
        }
        let conn = self.lock_conn()?;
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = conn
            .prepare(
                "SELECT id, channel_id, sender_id, sender_name, content, timestamp, edited,
                        reply_to_message_id, reply_to_sender_name, reply_preview, pinned, link_url
                 FROM messages WHERE channel_id = ?1 AND content LIKE ?2 ESCAPE '\\'
                 ORDER BY timestamp DESC LIMIT ?3",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![channel_id, pattern, limit as i64], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    sender_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    content: row.get(4)?,
                    timestamp: row.get(5)?,
                    edited: row.get::<_, i64>(6)? != 0,
                    reply_to_message_id: row.get(7)?,
                    reply_to_sender_name: row.get(8)?,
                    reply_preview: row.get(9)?,
                    pinned: row.get::<_, i64>(10)? != 0,
                    link_url: row.get(11)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        let mut msgs: Vec<_> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))?;
        msgs.reverse();
        Ok(msgs)
    }

    /// Search messages across multiple channels (space-wide search).
    pub fn search_messages_multi(
        &self,
        channel_ids: &[String],
        query: &str,
        limit: u32,
    ) -> Result<Vec<MessageRow>, String> {
        if query.len() > 256 || channel_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock_conn()?;
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        // Build IN clause with positional params
        let placeholders: Vec<String> = (0..channel_ids.len())
            .map(|i| format!("?{}", i + 3))
            .collect();
        let sql = format!(
            "SELECT id, channel_id, sender_id, sender_name, content, timestamp, edited,
                    reply_to_message_id, reply_to_sender_name, reply_preview, pinned, link_url
             FROM messages WHERE channel_id IN ({}) AND content LIKE ?1 ESCAPE '\\'
             ORDER BY timestamp DESC LIMIT ?2",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("Query error: {e}"))?;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        params_vec.push(Box::new(pattern));
        params_vec.push(Box::new(limit as i64));
        for cid in channel_ids {
            params_vec.push(Box::new(cid.clone()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    sender_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    content: row.get(4)?,
                    timestamp: row.get(5)?,
                    edited: row.get::<_, i64>(6)? != 0,
                    reply_to_message_id: row.get(7)?,
                    reply_to_sender_name: row.get(8)?,
                    reply_preview: row.get(9)?,
                    pinned: row.get::<_, i64>(10)? != 0,
                    link_url: row.get(11)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        let mut msgs: Vec<_> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))?;
        msgs.reverse();
        Ok(msgs)
    }

    pub fn get_message_sender(&self, message_id: &str) -> Result<Option<String>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT sender_id FROM messages WHERE id = ?1")
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt.query_row(params![message_id], |row| row.get(0)).ok();
        Ok(result)
    }

    // ─── Users / Auth ───

    pub fn save_user(&self, user: &UserRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO users (user_id, token, display_name, created_at, issued_at, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                user.user_id,
                user.token,
                user.display_name,
                user.created_at,
                user.issued_at,
                user.last_seen_at
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn find_user_by_token(&self, token: &str) -> Result<Option<UserRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT user_id, token, display_name, created_at, issued_at, last_seen_at FROM users WHERE token = ?1",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![token], |row| {
                Ok(UserRow {
                    user_id: row.get(0)?,
                    token: row.get(1)?,
                    display_name: row.get(2)?,
                    created_at: row.get(3)?,
                    issued_at: row.get(4)?,
                    last_seen_at: row.get(5)?,
                })
            })
            .ok();
        // Check token expiry: tokens older than TOKEN_LIFETIME_SECS are treated as invalid.
        // The caller (auth handler) will fall through to creating a new identity.
        if let Some(ref user) = result {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if user.issued_at + TOKEN_LIFETIME_SECS < now {
                log::info!("Token expired for user {}", user.user_id);
                return Ok(None);
            }
        }
        Ok(result)
    }

    pub fn find_user_by_id(&self, user_id: &str) -> Result<Option<UserRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT user_id, token, display_name, created_at, issued_at, last_seen_at FROM users WHERE user_id = ?1",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![user_id], |row| {
                Ok(UserRow {
                    user_id: row.get(0)?,
                    token: row.get(1)?,
                    display_name: row.get(2)?,
                    created_at: row.get(3)?,
                    issued_at: row.get(4)?,
                    last_seen_at: row.get(5)?,
                })
            })
            .ok();
        Ok(result)
    }

    pub fn find_user_by_display_name(&self, name: &str) -> Result<Option<UserRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT user_id, token, display_name, created_at, issued_at, last_seen_at FROM users WHERE display_name = ?1 COLLATE NOCASE",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![name], |row| {
                Ok(UserRow {
                    user_id: row.get(0)?,
                    token: row.get(1)?,
                    display_name: row.get(2)?,
                    created_at: row.get(3)?,
                    issued_at: row.get(4)?,
                    last_seen_at: row.get(5)?,
                })
            })
            .ok();
        Ok(result)
    }

    pub fn update_user_name(&self, user_id: &str, name: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE users SET display_name = ?2 WHERE user_id = ?1",
            params![user_id, name],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    pub fn update_last_seen(&self, user_id: &str, last_seen_at: i64) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE users SET last_seen_at = ?2 WHERE user_id = ?1",
            params![user_id, last_seen_at],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    pub fn rotate_user_session(
        &self,
        user_id: &str,
        token: &str,
        name: &str,
        issued_at: i64,
        last_seen_at: i64,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE users
             SET token = ?2, display_name = ?3, issued_at = ?4, last_seen_at = ?5
             WHERE user_id = ?1",
            params![user_id, token, name, issued_at, last_seen_at],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    // ─── Account (email + password) ───

    /// Create an account with email and hashed password.
    pub fn create_account(
        &self,
        user_id: &str,
        email: &str,
        password_hash: &str,
        display_name: &str,
        token: &str,
        now: i64,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO users (user_id, token, display_name, created_at, issued_at, last_seen_at, email, password_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![user_id, token, display_name, now, now, now, email, password_hash],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                "An account with this email already exists".to_string()
            } else {
                format!("Insert error: {e}")
            }
        })?;
        Ok(())
    }

    /// Find a user by email for login.
    pub fn find_user_by_email(&self, email: &str) -> Result<Option<(UserRow, String)>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT user_id, token, display_name, created_at, issued_at, last_seen_at, password_hash
                 FROM users WHERE email = ?1",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![email], |row| {
                Ok((
                    UserRow {
                        user_id: row.get(0)?,
                        token: row.get(1)?,
                        display_name: row.get(2)?,
                        created_at: row.get(3)?,
                        issued_at: row.get(4)?,
                        last_seen_at: row.get(5)?,
                    },
                    row.get::<_, String>(6)?,
                ))
            })
            .ok();
        Ok(result)
    }

    /// Get the password hash for a user (for change password).
    pub fn get_password_hash(&self, user_id: &str) -> Result<Option<String>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT password_hash FROM users WHERE user_id = ?1")
            .map_err(|e| format!("Query error: {e}"))?;
        let result: Option<Option<String>> = stmt
            .query_row(params![user_id], |row| row.get(0))
            .ok();
        Ok(result.flatten())
    }

    /// Update the password hash for a user.
    pub fn update_password_hash(
        &self,
        user_id: &str,
        password_hash: &str,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE users SET password_hash = ?2 WHERE user_id = ?1",
            params![user_id, password_hash],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    /// Invalidate token (logout). Replaces token with a unique revoked marker so no
    /// future lookup can match it, while preserving the NOT NULL UNIQUE constraint.
    pub fn invalidate_token(&self, user_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        // Use a unique revoked marker: "revoked:<user_id>" — this can never match a
        // valid hex token and is unique per user.
        let revoked = format!("revoked:{user_id}");
        conn.execute(
            "UPDATE users SET token = ?2 WHERE user_id = ?1",
            params![user_id, revoked],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    // ─── Friends ───

    pub fn save_friend_request(&self, request: &FriendRequestRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO friend_requests (requester_id, addressee_id, created_at)
             VALUES (?1, ?2, ?3)",
            params![
                request.requester_id,
                request.addressee_id,
                request.created_at
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn delete_friend_request(
        &self,
        requester_id: &str,
        addressee_id: &str,
    ) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "DELETE FROM friend_requests WHERE requester_id = ?1 AND addressee_id = ?2",
                params![requester_id, addressee_id],
            )
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn friend_request_exists(
        &self,
        requester_id: &str,
        addressee_id: &str,
    ) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT 1 FROM friend_requests WHERE requester_id = ?1 AND addressee_id = ?2")
            .map_err(|e| format!("Query error: {e}"))?;
        let exists = stmt
            .query_row(params![requester_id, addressee_id], |_| Ok(()))
            .is_ok();
        Ok(exists)
    }

    pub fn load_incoming_friend_requests(
        &self,
        user_id: &str,
    ) -> Result<Vec<FriendRequestRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT requester_id, addressee_id, created_at
                 FROM friend_requests WHERE addressee_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(FriendRequestRow {
                    requester_id: row.get(0)?,
                    addressee_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn load_outgoing_friend_requests(
        &self,
        user_id: &str,
    ) -> Result<Vec<FriendRequestRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT requester_id, addressee_id, created_at
                 FROM friend_requests WHERE requester_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(FriendRequestRow {
                    requester_id: row.get(0)?,
                    addressee_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn save_friendship(&self, friendship: &FriendshipRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO friendships (user_low_id, user_high_id, created_at)
             VALUES (?1, ?2, ?3)",
            params![
                friendship.user_low_id,
                friendship.user_high_id,
                friendship.created_at
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn delete_friendship(&self, user_a: &str, user_b: &str) -> Result<bool, String> {
        let (low, high) = ordered_friend_pair(user_a, user_b);
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "DELETE FROM friendships WHERE user_low_id = ?1 AND user_high_id = ?2",
                params![low, high],
            )
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn friendship_exists(&self, user_a: &str, user_b: &str) -> Result<bool, String> {
        let (low, high) = ordered_friend_pair(user_a, user_b);
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT 1 FROM friendships WHERE user_low_id = ?1 AND user_high_id = ?2")
            .map_err(|e| format!("Query error: {e}"))?;
        let exists = stmt.query_row(params![low, high], |_| Ok(())).is_ok();
        Ok(exists)
    }

    pub fn load_friendships_for_user(&self, user_id: &str) -> Result<Vec<FriendshipRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT user_low_id, user_high_id, created_at
                 FROM friendships WHERE user_low_id = ?1 OR user_high_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(FriendshipRow {
                    user_low_id: row.get(0)?,
                    user_high_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    // ─── Direct Messages ───

    pub fn load_direct_messages_between(
        &self,
        user_a: &str,
        user_b: &str,
        limit: usize,
    ) -> Result<Vec<DirectMessageRow>, String> {
        let (low, high) = ordered_friend_pair(user_a, user_b);
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_low_id, user_high_id, sender_user_id, sender_name, content,
                        timestamp, edited, reply_to_message_id, reply_to_sender_name, reply_preview
                 FROM direct_messages
                 WHERE user_low_id = ?1 AND user_high_id = ?2
                 ORDER BY timestamp DESC LIMIT ?3",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![low, high, limit as i64], |row| {
                Ok(DirectMessageRow {
                    id: row.get(0)?,
                    user_low_id: row.get(1)?,
                    user_high_id: row.get(2)?,
                    sender_user_id: row.get(3)?,
                    sender_name: row.get(4)?,
                    content: row.get(5)?,
                    timestamp: row.get(6)?,
                    edited: row.get::<_, i64>(7)? != 0,
                    reply_to_message_id: row.get(8)?,
                    reply_to_sender_name: row.get(9)?,
                    reply_preview: row.get(10)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        let mut messages = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))?;
        messages.reverse();
        Ok(messages)
    }

    pub fn save_direct_message(&self, msg: &DirectMessageRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO direct_messages (
                id, user_low_id, user_high_id, sender_user_id, sender_name, content, timestamp,
                edited, reply_to_message_id, reply_to_sender_name, reply_preview
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                msg.id,
                msg.user_low_id,
                msg.user_high_id,
                msg.sender_user_id,
                msg.sender_name,
                msg.content,
                msg.timestamp,
                msg.edited as i64,
                msg.reply_to_message_id,
                msg.reply_to_sender_name,
                msg.reply_preview,
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn update_direct_message(
        &self,
        message_id: &str,
        new_content: &str,
    ) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE direct_messages SET content = ?2, edited = 1 WHERE id = ?1",
                params![message_id, new_content],
            )
            .map_err(|e| format!("Update error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn delete_direct_message(&self, message_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "DELETE FROM direct_messages WHERE id = ?1",
                params![message_id],
            )
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn get_direct_message(&self, message_id: &str) -> Result<Option<DirectMessageRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_low_id, user_high_id, sender_user_id, sender_name, content,
                        timestamp, edited, reply_to_message_id, reply_to_sender_name, reply_preview
                 FROM direct_messages WHERE id = ?1",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![message_id], |row| {
                Ok(DirectMessageRow {
                    id: row.get(0)?,
                    user_low_id: row.get(1)?,
                    user_high_id: row.get(2)?,
                    sender_user_id: row.get(3)?,
                    sender_name: row.get(4)?,
                    content: row.get(5)?,
                    timestamp: row.get(6)?,
                    edited: row.get::<_, i64>(7)? != 0,
                    reply_to_message_id: row.get(8)?,
                    reply_to_sender_name: row.get(9)?,
                    reply_preview: row.get(10)?,
                })
            })
            .ok();
        Ok(result)
    }

    // ─── Bans ───

    pub fn save_ban(&self, ban: &BanRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO bans (space_id, user_id, banned_at) VALUES (?1, ?2, ?3)",
            params![ban.space_id, ban.user_id, ban.banned_at],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn is_banned(&self, space_id: &str, user_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT 1 FROM bans WHERE space_id = ?1 AND user_id = ?2")
            .map_err(|e| format!("Query error: {e}"))?;
        let exists = stmt
            .query_row(params![space_id, user_id], |_| Ok(()))
            .is_ok();
        Ok(exists)
    }

    pub fn load_bans_for_space(&self, space_id: &str) -> Result<Vec<BanRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT space_id, user_id, banned_at FROM bans WHERE space_id = ?1")
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![space_id], |row| {
                Ok(BanRow {
                    space_id: row.get(0)?,
                    user_id: row.get(1)?,
                    banned_at: row.get(2)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn save_space_role(&self, role: &SpaceRoleRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO space_roles (space_id, user_id, role, assigned_at, role_color)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![role.space_id, role.user_id, role.role, role.assigned_at, role.role_color],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn delete_space_role(&self, space_id: &str, user_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "DELETE FROM space_roles WHERE space_id = ?1 AND user_id = ?2",
                params![space_id, user_id],
            )
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn load_space_roles(&self, space_id: &str) -> Result<Vec<SpaceRoleRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT space_id, user_id, role, assigned_at,
                        COALESCE(role_color, '') as role_color
                 FROM space_roles WHERE space_id = ?1 ORDER BY assigned_at ASC",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![space_id], |row| {
                Ok(SpaceRoleRow {
                    space_id: row.get(0)?,
                    user_id: row.get(1)?,
                    role: row.get(2)?,
                    assigned_at: row.get(3)?,
                    role_color: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    /// Set the display color for a role in a space. `color` is a hex string like "#ff5555".
    pub fn set_role_color(
        &self,
        space_id: &str,
        role: &str,
        color: &str,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE space_roles SET role_color = ?1 WHERE space_id = ?2 AND role = ?3",
            params![color, space_id, role],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    /// Get the role color for a specific role in a space.
    pub fn get_role_color(&self, space_id: &str, role: &str) -> String {
        let Ok(conn) = self.lock_conn() else {
            return String::new();
        };
        conn.query_row(
            "SELECT COALESCE(role_color, '') FROM space_roles WHERE space_id = ?1 AND role = ?2 LIMIT 1",
            params![space_id, role],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
    }

    pub fn save_audit_log_entry(&self, entry: &AuditLogRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO space_audit_log (
                id, space_id, actor_user_id, actor_name, action, target_user_id, target_name, detail, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.id,
                entry.space_id,
                entry.actor_user_id,
                entry.actor_name,
                entry.action,
                entry.target_user_id,
                entry.target_name,
                entry.detail,
                entry.created_at,
            ],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn load_audit_log_for_space(
        &self,
        space_id: &str,
        limit: usize,
    ) -> Result<Vec<AuditLogRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, space_id, actor_user_id, actor_name, action, target_user_id, target_name, detail, created_at
                 FROM space_audit_log
                 WHERE space_id = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![space_id, limit as i64], |row| {
                Ok(AuditLogRow {
                    id: row.get(0)?,
                    space_id: row.get(1)?,
                    actor_user_id: row.get(2)?,
                    actor_name: row.get(3)?,
                    action: row.get(4)?,
                    target_user_id: row.get(5)?,
                    target_name: row.get(6)?,
                    detail: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    /// Get the highest numeric suffix from space/channel/message IDs to restore allocators.
    pub fn max_id_suffix(&self, table: &str, col: &str) -> Result<u64, String> {
        let conn = self.lock_conn()?;
        // table and col are controlled internally, not from user input
        let query = format!("SELECT {col} FROM {table}");
        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                let val: String = row.get(0)?;
                Ok(val)
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut max_val: u64 = 0;
        for val in rows.flatten() {
            // Extract numeric suffix after prefix char (e.g. "s12" -> 12)
            if let Some(num_str) = val.get(1..) {
                if let Ok(n) = num_str.parse::<u64>() {
                    max_val = max_val.max(n);
                }
            }
        }
        Ok(max_val)
    }
}

// ─── v0.8.0: New Database Methods ───

impl Database {
    pub fn delete_ban(&self, space_id: &str, user_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let changes = conn
            .execute(
                "DELETE FROM bans WHERE space_id = ?1 AND user_id = ?2",
                params![space_id, user_id],
            )
            .map_err(|e| format!("Failed to delete ban: {e}"))?;
        Ok(changes > 0)
    }

    pub fn load_bans(&self, space_id: &str) -> Result<Vec<BanRow>, String> {
        self.load_bans_for_space(space_id)
    }

    pub fn save_user_block(&self, blocker_id: &str, blocked_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO user_blocks (blocker_id, blocked_id, created_at) VALUES (?1, ?2, ?3)",
            params![blocker_id, blocked_id, now],
        )
        .map_err(|e| format!("Failed to save block: {e}"))?;
        Ok(())
    }

    pub fn delete_user_block(&self, blocker_id: &str, blocked_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM user_blocks WHERE blocker_id = ?1 AND blocked_id = ?2",
            params![blocker_id, blocked_id],
        )
        .map_err(|e| format!("Failed to delete block: {e}"))?;
        Ok(())
    }

    pub fn is_blocked(&self, blocker_id: &str, blocked_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_blocks WHERE blocker_id = ?1 AND blocked_id = ?2",
                params![blocker_id, blocked_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check block: {e}"))?;
        Ok(count > 0)
    }

    /// Get all user_ids that have blocked the given user.
    /// Used to populate the blocked_by cache on authentication.
    pub fn get_users_who_blocked(&self, blocked_id: &str) -> Result<Vec<String>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT blocker_id FROM user_blocks WHERE blocked_id = ?1")
            .map_err(|e| format!("Failed to query blocks: {e}"))?;
        let ids = stmt
            .query_map(params![blocked_id], |row| row.get(0))
            .map_err(|e| format!("Failed to fetch blocks: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    pub fn create_group_conversation(
        &self,
        group_id: &str,
        name: &str,
        member_ids: &[String],
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO group_conversations (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![group_id, name, now],
        )
        .map_err(|e| format!("Failed to create group: {e}"))?;
        for uid in member_ids {
            conn.execute(
                "INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?1, ?2)",
                params![group_id, uid],
            )
            .map_err(|e| format!("Failed to add group member: {e}"))?;
        }
        Ok(())
    }

    pub fn is_group_member(&self, group_id: &str, user_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND user_id = ?2",
                params![group_id, user_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check group membership: {e}"))?;
        Ok(count > 0)
    }

    pub fn load_group_members(&self, group_id: &str) -> Result<Vec<String>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT user_id FROM group_members WHERE group_id = ?1")
            .map_err(|e| format!("Failed to prepare: {e}"))?;
        let rows = stmt
            .query_map(params![group_id], |row| row.get(0))
            .map_err(|e| format!("Failed to query: {e}"))?;
        let mut members = Vec::new();
        for row in rows {
            members.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(members)
    }

    pub fn load_group_name(&self, group_id: &str) -> Result<String, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT name FROM group_conversations WHERE id = ?1",
            params![group_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Group not found: {e}"))
    }

    pub fn load_group_messages(
        &self,
        group_id: &str,
        limit: usize,
    ) -> Result<Vec<GroupMessageRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, group_id, sender_id, sender_name, content, timestamp, edited,
                        reply_to_message_id, reply_to_sender_name, reply_preview
                 FROM group_messages WHERE group_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
            )
            .map_err(|e| format!("Failed to prepare: {e}"))?;
        let rows = stmt
            .query_map(params![group_id, limit as i64], |row| {
                Ok(GroupMessageRow {
                    id: row.get(0)?,
                    group_id: row.get(1)?,
                    sender_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    content: row.get(4)?,
                    timestamp: row.get(5)?,
                    edited: row.get::<_, i64>(6)? != 0,
                    reply_to_message_id: row.get(7)?,
                    reply_to_sender_name: row.get(8)?,
                    reply_preview: row.get(9)?,
                })
            })
            .map_err(|e| format!("Failed to query: {e}"))?;
        let mut messages: Vec<GroupMessageRow> = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        messages.reverse(); // ascending order
        Ok(messages)
    }

    pub fn save_group_message(&self, msg: &GroupMessageRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO group_messages (id, group_id, sender_id, sender_name, content, timestamp, edited, reply_to_message_id, reply_to_sender_name, reply_preview) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                msg.id, msg.group_id, msg.sender_id, msg.sender_name,
                msg.content, msg.timestamp, msg.edited as i64,
                msg.reply_to_message_id, msg.reply_to_sender_name, msg.reply_preview
            ],
        )
        .map_err(|e| format!("Failed to save group message: {e}"))?;
        Ok(())
    }

    pub fn set_invite_settings(
        &self,
        space_id: &str,
        expires_hours: Option<u32>,
        max_uses: Option<u32>,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        let expires_at: Option<i64> = expires_hours.map(|h| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
                + (h as i64 * 3600)
        });
        conn.execute(
            "UPDATE spaces SET invite_expires_at = ?1, invite_max_uses = ?2 WHERE id = ?3",
            params![expires_at, max_uses.map(|u| u as i64), space_id],
        )
        .map_err(|e| format!("Failed to update invite settings: {e}"))?;
        Ok(())
    }

    pub fn set_space_nickname(
        &self,
        space_id: &str,
        user_id: &str,
        nickname: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        match nickname {
            Some(nick) => {
                conn.execute(
                    "INSERT OR REPLACE INTO space_nicknames (space_id, user_id, nickname) VALUES (?1, ?2, ?3)",
                    params![space_id, user_id, nick],
                )
                .map_err(|e| format!("Failed to set nickname: {e}"))?;
            }
            None => {
                conn.execute(
                    "DELETE FROM space_nicknames WHERE space_id = ?1 AND user_id = ?2",
                    params![space_id, user_id],
                )
                .map_err(|e| format!("Failed to remove nickname: {e}"))?;
            }
        }
        Ok(())
    }
}

// ─── Auto-moderation ───

#[derive(Debug, Clone)]
pub struct AutomodFilterRow {
    pub space_id: String,
    pub word: String,
    pub action: String,
}

impl Database {
    pub fn add_automod_word(
        &self,
        space_id: &str,
        word: &str,
        action: &str,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO automod_filters (space_id, word, action) VALUES (?1, ?2, ?3)",
            params![space_id, word.to_lowercase(), action],
        )
        .map_err(|e| format!("Failed to add automod word: {e}"))?;
        Ok(())
    }

    pub fn remove_automod_word(&self, space_id: &str, word: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "DELETE FROM automod_filters WHERE space_id = ?1 AND word = ?2",
                params![space_id, word.to_lowercase()],
            )
            .map_err(|e| format!("Failed to remove automod word: {e}"))?;
        Ok(rows > 0)
    }

    pub fn load_automod_words(&self, space_id: &str) -> Result<Vec<AutomodFilterRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT space_id, word, action FROM automod_filters WHERE space_id = ?1 ORDER BY word ASC",
            )
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![space_id], |row| {
                Ok(AutomodFilterRow {
                    space_id: row.get(0)?,
                    word: row.get(1)?,
                    action: row.get(2)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    // ── Account Management ──

    pub fn update_display_name(&self, user_id: &str, name: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute("UPDATE users SET display_name = ?1 WHERE user_id = ?2", rusqlite::params![name, user_id])
            .map_err(|e| format!("update name: {e}"))?;
        Ok(())
    }

    pub fn delete_user(&self, user_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute("DELETE FROM users WHERE user_id = ?1", [user_id])
            .map_err(|e| format!("delete user: {e}"))?;
        conn.execute("DELETE FROM friendships WHERE user_a = ?1 OR user_b = ?1", [user_id])
            .map_err(|e| format!("del friends: {e}"))?;
        conn.execute("DELETE FROM friend_requests WHERE from_id = ?1 OR to_id = ?1", [user_id])
            .map_err(|e| format!("del requests: {e}"))?;
        conn.execute("DELETE FROM user_blocks WHERE blocker_id = ?1 OR blocked_id = ?1", [user_id])
            .map_err(|e| format!("del blocks: {e}"))?;
        Ok(())
    }

    // ── Scheduled Events ──

    pub fn create_scheduled_event(
        &self, id: &str, space_id: &str, title: &str, description: &str,
        start_time: i64, end_time: i64, creator_id: &str, creator_name: &str,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO scheduled_events (id, space_id, title, description, start_time, end_time, creator_id, creator_name) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![id, space_id, title, description, start_time, end_time, creator_id, creator_name],
        ).map_err(|e| format!("create event: {e}"))?;
        Ok(())
    }

    pub fn delete_scheduled_event(&self, event_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute("DELETE FROM event_interests WHERE event_id = ?1", [event_id])
            .map_err(|e| format!("del interests: {e}"))?;
        conn.execute("DELETE FROM scheduled_events WHERE id = ?1", [event_id])
            .map_err(|e| format!("del event: {e}"))?;
        Ok(())
    }

    pub fn toggle_event_interest(&self, event_id: &str, user_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM event_interests WHERE event_id=?1 AND user_id=?2",
                [event_id, user_id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| format!("check interest: {e}"))? > 0;
        if exists {
            conn.execute(
                "DELETE FROM event_interests WHERE event_id=?1 AND user_id=?2",
                [event_id, user_id],
            ).map_err(|e| format!("rm interest: {e}"))?;
            Ok(false)
        } else {
            conn.execute(
                "INSERT INTO event_interests (event_id, user_id) VALUES (?1, ?2)",
                [event_id, user_id],
            ).map_err(|e| format!("add interest: {e}"))?;
            Ok(true)
        }
    }

    pub fn load_scheduled_events(&self, space_id: &str, viewer_user_id: &str) -> Result<Vec<shared_types::ScheduledEvent>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT e.id, e.title, e.description, e.start_time, e.end_time, e.creator_name,
                    (SELECT COUNT(*) FROM event_interests WHERE event_id = e.id) as cnt,
                    (SELECT COUNT(*) FROM event_interests WHERE event_id = e.id AND user_id = ?2) as me
             FROM scheduled_events e WHERE e.space_id = ?1 ORDER BY e.start_time ASC"
        ).map_err(|e| format!("prepare events: {e}"))?;
        let rows = stmt.query_map(rusqlite::params![space_id, viewer_user_id], |r| {
            Ok(shared_types::ScheduledEvent {
                id: r.get(0)?,
                title: r.get(1)?,
                description: r.get(2)?,
                start_time: r.get(3)?,
                end_time: r.get(4)?,
                creator_name: r.get(5)?,
                interested_count: r.get::<_, i64>(6)? as u32,
                is_interested: r.get::<_, i64>(7)? > 0,
            })
        }).map_err(|e| format!("query events: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("row: {e}"))?);
        }
        Ok(out)
    }

    pub fn get_event_interest_count(&self, event_id: &str) -> Result<u32, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM event_interests WHERE event_id = ?1",
            [event_id],
            |r| r.get::<_, i64>(0),
        ).map(|c| c as u32).map_err(|e| format!("count: {e}"))
    }

    // ── Scheduled Messages ──

    pub fn schedule_message(
        &self, id: &str, space_id: &str, channel_id: &str, sender_id: &str,
        sender_name: &str, content: &str, send_at: i64,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        conn.execute(
            "INSERT INTO scheduled_messages (id, space_id, channel_id, sender_id, sender_name, content, send_at, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![id, space_id, channel_id, sender_id, sender_name, content, send_at, now],
        ).map_err(|e| format!("schedule msg: {e}"))?;
        Ok(())
    }

    pub fn cancel_scheduled_message(&self, schedule_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute("DELETE FROM scheduled_messages WHERE id = ?1", [schedule_id])
            .map_err(|e| format!("cancel sched: {e}"))?;
        Ok(())
    }

    pub fn get_due_scheduled_messages(&self) -> Result<Vec<(String, String, String, String, String, String)>, String> {
        let conn = self.lock_conn()?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let mut stmt = conn.prepare(
            "SELECT id, space_id, channel_id, sender_id, sender_name, content FROM scheduled_messages WHERE send_at <= ?1"
        ).map_err(|e| format!("prep due: {e}"))?;
        let rows = stmt.query_map([now], |r: &rusqlite::Row| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))
        }).map_err(|e| format!("query due: {e}"))?;
        let mut out: Vec<(String, String, String, String, String, String)> = Vec::new();
        for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
        Ok(out)
    }

    pub fn delete_scheduled_message(&self, id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute("DELETE FROM scheduled_messages WHERE id = ?1", [id])
            .map_err(|e| format!("del sched: {e}"))?;
        Ok(())
    }

    /// Return the sender_id of a scheduled message, or None if not found.
    pub fn get_scheduled_message_sender(&self, schedule_id: &str) -> Result<Option<String>, String> {
        let conn = self.lock_conn()?;
        match conn.query_row(
            "SELECT sender_id FROM scheduled_messages WHERE id = ?1",
            [schedule_id],
            |r| r.get::<_, String>(0),
        ) {
            Ok(sender_id) => Ok(Some(sender_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("get sched sender: {e}")),
        }
    }

    /// Return invite settings for a space: (invite_expires_at, invite_max_uses, invite_uses).
    pub fn get_invite_info(&self, space_id: &str) -> Result<(Option<i64>, Option<i64>, i64), String> {
        let conn = self.lock_conn()?;
        let result = conn.query_row(
            "SELECT invite_expires_at, invite_max_uses, COALESCE(invite_uses, 0) FROM spaces WHERE id = ?1",
            params![space_id],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, i64>(2)?)),
        ).map_err(|e| format!("get invite info: {e}"))?;
        Ok(result)
    }

    /// Increment invite_uses counter for a space.
    pub fn increment_invite_uses(&self, space_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE spaces SET invite_uses = COALESCE(invite_uses, 0) + 1 WHERE id = ?1",
            params![space_id],
        ).map_err(|e| format!("inc invite uses: {e}"))?;
        Ok(())
    }

    // ── Welcome Message ──

    pub fn set_welcome_message(&self, space_id: &str, message: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE spaces SET welcome_message = ?1 WHERE id = ?2",
            rusqlite::params![message, space_id],
        ).map_err(|e| format!("set welcome: {e}"))?;
        Ok(())
    }

    pub fn get_welcome_message(&self, space_id: &str) -> Result<String, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT COALESCE(welcome_message, '') FROM spaces WHERE id = ?1",
            [space_id],
            |r| r.get::<_, String>(0),
        ).map_err(|e| format!("get welcome: {e}"))
    }

    // ── Server Discovery ──

    pub fn is_space_public(&self, space_id: &str) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let val: i32 = conn
            .query_row(
                "SELECT COALESCE(is_public, 0) FROM spaces WHERE id = ?1",
                params![space_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("is_space_public: {e}"))?;
        Ok(val != 0)
    }

    pub fn set_space_public(&self, space_id: &str, is_public: bool) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE spaces SET is_public = ?1 WHERE id = ?2",
            params![is_public as i32, space_id],
        ).map_err(|e| format!("set public: {e}"))?;
        Ok(())
    }

    pub fn load_public_spaces(&self) -> Result<Vec<(String, String, String, String)>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, COALESCE(description, ''), invite_code FROM spaces WHERE is_public = 1"
        ).map_err(|e| format!("prep public: {e}"))?;
        let rows = stmt.query_map([], |r: &rusqlite::Row| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        }).map_err(|e| format!("query public: {e}"))?;
        let mut out = Vec::new();
        for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
        Ok(out)
    }
}

fn ordered_friend_pair<'a>(user_a: &'a str, user_b: &'a str) -> (&'a str, &'a str) {
    if user_a <= user_b {
        (user_a, user_b)
    } else {
        (user_b, user_a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db() -> (Database, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("voxlink_test_{}_{n}.db", std::process::id()));
        let db = Database::open(&path).unwrap();
        (db, path)
    }

    #[test]
    fn space_round_trip() {
        let (db, path) = temp_db();
        let space = SpaceRow {
            id: "s1".into(),
            name: "Test".into(),
            invite_code: "ABC123".into(),
            owner_id: "p1".into(),
            created_at: 1000,
        };
        db.save_space(&space).unwrap();
        let spaces = db.load_all_spaces().unwrap();
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].name, "Test");
        assert_eq!(spaces[0].invite_code, "ABC123");

        db.delete_space("s1").unwrap();
        assert!(db.load_all_spaces().unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn delete_space_cascades_space_data() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "Alpha".into(),
            invite_code: "ALPHA123".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        db.save_space(&SpaceRow {
            id: "s2".into(),
            name: "Beta".into(),
            invite_code: "BETA1234".into(),
            owner_id: "p2".into(),
            created_at: 1,
        })
        .unwrap();
        db.save_channel(&ChannelRow {
            id: "c1".into(),
            space_id: "s1".into(),
            name: "general".into(),
            room_key: "sp:s1:ch:c1".into(),
            channel_type: "text".into(),
            topic: None,
            voice_quality: None,
            min_role: None,
            position: None,
            auto_delete_hours: None,
        })
        .unwrap();
        db.save_channel(&ChannelRow {
            id: "c2".into(),
            space_id: "s2".into(),
            name: "general".into(),
            room_key: "sp:s2:ch:c2".into(),
            channel_type: "text".into(),
            topic: None,
            voice_quality: None,
            min_role: None,
            position: None,
            auto_delete_hours: None,
        })
        .unwrap();
        db.save_message(&MessageRow {
            id: "m1".into(),
            channel_id: "c1".into(),
            sender_id: "p1".into(),
            sender_name: "Alice".into(),
            content: "hello".into(),
            timestamp: 10,
            edited: false,
            reply_to_message_id: None,
            reply_to_sender_name: None,
            reply_preview: None,
            pinned: false,
            link_url: None,
        })
        .unwrap();
        db.save_message(&MessageRow {
            id: "m2".into(),
            channel_id: "c2".into(),
            sender_id: "p2".into(),
            sender_name: "Bob".into(),
            content: "still here".into(),
            timestamp: 11,
            edited: false,
            reply_to_message_id: None,
            reply_to_sender_name: None,
            reply_preview: None,
            pinned: false,
            link_url: None,
        })
        .unwrap();
        db.save_ban(&BanRow {
            space_id: "s1".into(),
            user_id: "u1".into(),
            banned_at: 12,
        })
        .unwrap();
        db.save_ban(&BanRow {
            space_id: "s2".into(),
            user_id: "u2".into(),
            banned_at: 13,
        })
        .unwrap();

        db.delete_space("s1").unwrap();

        let spaces = db.load_all_spaces().unwrap();
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].id, "s2");
        assert!(db.load_channels_for_space("s1").unwrap().is_empty());
        assert!(db.load_messages_for_channel("c1", 50).unwrap().is_empty());
        assert!(!db.is_banned("s1", "u1").unwrap());
        assert_eq!(db.load_channels_for_space("s2").unwrap().len(), 1);
        assert_eq!(db.load_messages_for_channel("c2", 50).unwrap().len(), 1);
        assert!(db.is_banned("s2", "u2").unwrap());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn channel_round_trip() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        db.save_channel(&ChannelRow {
            id: "c1".into(),
            space_id: "s1".into(),
            name: "General".into(),
            room_key: "sp:s1:ch:c1".into(),
            channel_type: "voice".into(),
            topic: None,
            voice_quality: None,
            min_role: None,
            position: None,
            auto_delete_hours: None,
        })
        .unwrap();
        let channels = db.load_channels_for_space("s1").unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "General");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn delete_channel_removes_messages_only_for_target() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        for (id, name) in [("c1", "General"), ("c2", "Side")] {
            db.save_channel(&ChannelRow {
                id: id.into(),
                space_id: "s1".into(),
                name: name.into(),
                room_key: format!("sp:s1:ch:{id}"),
                channel_type: "text".into(),
                topic: None,
                voice_quality: None,
                min_role: None,
                position: None,
                auto_delete_hours: None,
            })
            .unwrap();
        }
        for (id, channel_id, content) in [("m1", "c1", "remove me"), ("m2", "c2", "keep me")] {
            db.save_message(&MessageRow {
                id: id.into(),
                channel_id: channel_id.into(),
                sender_id: "p1".into(),
                sender_name: "Alice".into(),
                content: content.into(),
                timestamp: 100,
                edited: false,
                reply_to_message_id: None,
                reply_to_sender_name: None,
                reply_preview: None,
                pinned: false,
                link_url: None,
            })
            .unwrap();
        }

        db.delete_channel("c1").unwrap();

        let remaining_channels = db.load_channels_for_space("s1").unwrap();
        assert_eq!(remaining_channels.len(), 1);
        assert_eq!(remaining_channels[0].id, "c2");
        assert!(db.load_messages_for_channel("c1", 50).unwrap().is_empty());
        let remaining_messages = db.load_messages_for_channel("c2", 50).unwrap();
        assert_eq!(remaining_messages.len(), 1);
        assert_eq!(remaining_messages[0].content, "keep me");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn message_round_trip() {
        let (db, path) = temp_db();
        // Create parent space and channel for foreign key constraints
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        db.save_channel(&ChannelRow {
            id: "c1".into(),
            space_id: "s1".into(),
            name: "General".into(),
            room_key: "sp:s1:ch:c1".into(),
            channel_type: "text".into(),
            topic: None,
            voice_quality: None,
            min_role: None,
            position: None,
            auto_delete_hours: None,
        })
        .unwrap();
        db.save_message(&MessageRow {
            id: "m1".into(),
            channel_id: "c1".into(),
            sender_id: "p1".into(),
            sender_name: "Alice".into(),
            content: "Hello".into(),
            timestamp: 1000,
            edited: false,
            reply_to_message_id: Some("m0".into()),
            reply_to_sender_name: Some("Bob".into()),
            reply_preview: Some("Earlier note".into()),
            pinned: true,
            link_url: None,
        })
        .unwrap();
        let msgs = db.load_messages_for_channel("c1", 50).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello");
        assert_eq!(msgs[0].reply_to_message_id.as_deref(), Some("m0"));
        assert_eq!(msgs[0].reply_to_sender_name.as_deref(), Some("Bob"));
        assert_eq!(msgs[0].reply_preview.as_deref(), Some("Earlier note"));
        assert!(msgs[0].pinned);

        db.update_message("m1", "Updated").unwrap();
        let msgs = db.load_messages_for_channel("c1", 50).unwrap();
        assert_eq!(msgs[0].content, "Updated");
        assert!(msgs[0].edited);

        db.set_message_pinned("m1", false).unwrap();
        let msgs = db.load_messages_for_channel("c1", 50).unwrap();
        assert!(!msgs[0].pinned);

        assert!(db.delete_message("m1").unwrap());
        assert!(db.load_messages_for_channel("c1", 50).unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn user_auth_round_trip() {
        let (db, path) = temp_db();
        // Use a recent timestamp so token expiry check passes
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        db.save_user(&UserRow {
            user_id: "u1".into(),
            token: "tok123".into(),
            display_name: "Alice".into(),
            created_at: now,
            issued_at: now,
            last_seen_at: now,
        })
        .unwrap();
        let user = db.find_user_by_token("tok123").unwrap().unwrap();
        assert_eq!(user.user_id, "u1");
        assert_eq!(user.display_name, "Alice");
        assert_eq!(user.issued_at, now);
        assert_eq!(user.last_seen_at, now);

        db.rotate_user_session("u1", "tok456", "Alice 2", now + 1000, now + 2000)
            .unwrap();
        let rotated = db.find_user_by_token("tok456").unwrap().unwrap();
        assert_eq!(rotated.display_name, "Alice 2");
        assert_eq!(rotated.issued_at, now + 1000);
        assert_eq!(rotated.last_seen_at, now + 2000);

        assert!(db.find_user_by_token("nonexistent").unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ban_round_trip() {
        let (db, path) = temp_db();
        // Create parent space for foreign key constraint
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        assert!(!db.is_banned("s1", "u1").unwrap());
        db.save_ban(&BanRow {
            space_id: "s1".into(),
            user_id: "u1".into(),
            banned_at: 1000,
        })
        .unwrap();
        assert!(db.is_banned("s1", "u1").unwrap());
        assert!(!db.is_banned("s1", "u2").unwrap());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn space_roles_and_audit_round_trip() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "Ops".into(),
            invite_code: "OPS12345".into(),
            owner_id: "u1".into(),
            created_at: 0,
        })
        .unwrap();
        db.save_space_role(&SpaceRoleRow {
            space_id: "s1".into(),
            user_id: "u2".into(),
            role: "admin".into(),
            assigned_at: 10,
            role_color: String::new(),
        })
        .unwrap();
        db.save_audit_log_entry(&AuditLogRow {
            id: "a1".into(),
            space_id: "s1".into(),
            actor_user_id: "u1".into(),
            actor_name: "Alice".into(),
            action: "role".into(),
            target_user_id: Some("u2".into()),
            target_name: Some("Bob".into()),
            detail: "Bob is now admin".into(),
            created_at: 11,
        })
        .unwrap();

        let roles = db.load_space_roles("s1").unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].user_id, "u2");
        assert_eq!(roles[0].role, "admin");

        let audit_entries = db.load_audit_log_for_space("s1", 10).unwrap();
        assert_eq!(audit_entries.len(), 1);
        assert_eq!(audit_entries[0].action, "role");
        assert_eq!(audit_entries[0].target_name.as_deref(), Some("Bob"));

        assert!(db.delete_space_role("s1", "u2").unwrap());
        assert!(db.load_space_roles("s1").unwrap().is_empty());

        db.delete_space("s1").unwrap();
        assert!(db.load_audit_log_for_space("s1", 10).unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn friend_request_round_trip() {
        let (db, path) = temp_db();
        db.save_friend_request(&FriendRequestRow {
            requester_id: "u1".into(),
            addressee_id: "u2".into(),
            created_at: 1000,
        })
        .unwrap();
        assert!(db.friend_request_exists("u1", "u2").unwrap());
        assert!(!db.friend_request_exists("u2", "u1").unwrap());
        let incoming = db.load_incoming_friend_requests("u2").unwrap();
        let outgoing = db.load_outgoing_friend_requests("u1").unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].requester_id, "u1");
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].addressee_id, "u2");
        assert!(db.delete_friend_request("u1", "u2").unwrap());
        assert!(!db.friend_request_exists("u1", "u2").unwrap());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn friendship_round_trip() {
        let (db, path) = temp_db();
        db.save_friendship(&FriendshipRow {
            user_low_id: "u1".into(),
            user_high_id: "u2".into(),
            created_at: 1000,
        })
        .unwrap();
        assert!(db.friendship_exists("u1", "u2").unwrap());
        assert!(db.friendship_exists("u2", "u1").unwrap());
        let friends = db.load_friendships_for_user("u2").unwrap();
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].user_low_id, "u1");
        assert!(db.delete_friendship("u2", "u1").unwrap());
        assert!(!db.friendship_exists("u1", "u2").unwrap());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn direct_message_round_trip() {
        let (db, path) = temp_db();
        db.save_direct_message(&DirectMessageRow {
            id: "m7".into(),
            user_low_id: "u1".into(),
            user_high_id: "u2".into(),
            sender_user_id: "u1".into(),
            sender_name: "Alice".into(),
            content: "Hello there".into(),
            timestamp: 1000,
            edited: false,
            reply_to_message_id: Some("m5".into()),
            reply_to_sender_name: Some("Bob".into()),
            reply_preview: Some("Earlier note".into()),
        })
        .unwrap();

        let history = db.load_direct_messages_between("u2", "u1", 50).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Hello there");
        assert_eq!(history[0].reply_to_message_id.as_deref(), Some("m5"));
        assert_eq!(history[0].reply_to_sender_name.as_deref(), Some("Bob"));
        assert_eq!(history[0].reply_preview.as_deref(), Some("Earlier note"));

        assert!(db.update_direct_message("m7", "Updated").unwrap());
        let stored = db.get_direct_message("m7").unwrap().unwrap();
        assert_eq!(stored.content, "Updated");
        assert!(stored.edited);

        assert!(db.delete_direct_message("m7").unwrap());
        assert!(db.get_direct_message("m7").unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn max_id_suffix_works() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s5".into(),
            name: "A".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        db.save_space(&SpaceRow {
            id: "s12".into(),
            name: "B".into(),
            invite_code: "Y".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();
        assert_eq!(db.max_id_suffix("spaces", "id").unwrap(), 12);
        let _ = std::fs::remove_file(path);
    }

    // ─── v0.8.0 tests ───

    #[test]
    fn user_blocks_round_trip() {
        let (db, path) = temp_db();
        // Initially not blocked
        assert!(!db.is_blocked("u1", "u2").unwrap());

        // Block
        db.save_user_block("u1", "u2").unwrap();
        assert!(db.is_blocked("u1", "u2").unwrap());
        // Direction matters
        assert!(!db.is_blocked("u2", "u1").unwrap());

        // Unblock
        db.delete_user_block("u1", "u2").unwrap();
        assert!(!db.is_blocked("u1", "u2").unwrap());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ban_save_load_delete() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();

        db.save_ban(&BanRow {
            space_id: "s1".into(),
            user_id: "u1".into(),
            banned_at: 1000,
        })
        .unwrap();
        db.save_ban(&BanRow {
            space_id: "s1".into(),
            user_id: "u2".into(),
            banned_at: 2000,
        })
        .unwrap();

        let bans = db.load_bans("s1").unwrap();
        assert_eq!(bans.len(), 2);
        assert!(db.is_banned("s1", "u1").unwrap());
        assert!(db.is_banned("s1", "u2").unwrap());

        // Delete one ban
        assert!(db.delete_ban("s1", "u1").unwrap());
        assert!(!db.is_banned("s1", "u1").unwrap());
        assert!(db.is_banned("s1", "u2").unwrap());
        let bans = db.load_bans("s1").unwrap();
        assert_eq!(bans.len(), 1);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn group_conversation_round_trip() {
        let (db, path) = temp_db();
        let members = vec!["u1".into(), "u2".into(), "u3".into()];
        db.create_group_conversation("g1", "Squad", &members)
            .unwrap();

        assert!(db.is_group_member("g1", "u1").unwrap());
        assert!(db.is_group_member("g1", "u2").unwrap());
        assert!(db.is_group_member("g1", "u3").unwrap());
        assert!(!db.is_group_member("g1", "u99").unwrap());

        let loaded_members = db.load_group_members("g1").unwrap();
        assert_eq!(loaded_members.len(), 3);

        let name = db.load_group_name("g1").unwrap();
        assert_eq!(name, "Squad");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn group_messages_round_trip() {
        let (db, path) = temp_db();
        let members = vec!["u1".into(), "u2".into()];
        db.create_group_conversation("g1", "Duo", &members)
            .unwrap();

        db.save_group_message(&GroupMessageRow {
            id: "gm1".into(),
            group_id: "g1".into(),
            sender_id: "u1".into(),
            sender_name: "Alice".into(),
            content: "Hey group!".into(),
            timestamp: 1000,
            edited: false,
            reply_to_message_id: None,
            reply_to_sender_name: None,
            reply_preview: None,
        })
        .unwrap();
        db.save_group_message(&GroupMessageRow {
            id: "gm2".into(),
            group_id: "g1".into(),
            sender_id: "u2".into(),
            sender_name: "Bob".into(),
            content: "Hello!".into(),
            timestamp: 2000,
            edited: false,
            reply_to_message_id: Some("gm1".into()),
            reply_to_sender_name: Some("Alice".into()),
            reply_preview: Some("Hey group!".into()),
        })
        .unwrap();

        let messages = db.load_group_messages("g1", 50).unwrap();
        assert_eq!(messages.len(), 2);
        // Should be in ascending order
        assert_eq!(messages[0].id, "gm1");
        assert_eq!(messages[1].id, "gm2");
        assert_eq!(messages[1].reply_to_message_id.as_deref(), Some("gm1"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn space_nickname_persistence() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();

        // Set a nickname
        db.set_space_nickname("s1", "u1", Some("Ally")).unwrap();
        // Set another
        db.set_space_nickname("s1", "u1", Some("NewNick")).unwrap();

        // Remove nickname
        db.set_space_nickname("s1", "u1", None).unwrap();
        // Should not error when removing a non-existent nickname
        db.set_space_nickname("s1", "u1", None).unwrap();

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invite_settings_persistence() {
        let (db, path) = temp_db();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "S".into(),
            invite_code: "X".into(),
            owner_id: "p1".into(),
            created_at: 0,
        })
        .unwrap();

        // Set invite settings with expiry and max uses
        db.set_invite_settings("s1", Some(24), Some(10)).unwrap();

        // Set without expiry
        db.set_invite_settings("s1", None, Some(5)).unwrap();

        // Clear both
        db.set_invite_settings("s1", None, None).unwrap();

        let _ = std::fs::remove_file(path);
    }
}
