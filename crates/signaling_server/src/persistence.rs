use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

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

impl Database {
    /// Lock the DB connection, recovering from poisoned mutex (a prior panic in a
    /// spawn_blocking task). This prevents a single DB error from crashing all future ops.
    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
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
                CREATE INDEX IF NOT EXISTS idx_direct_messages_timestamp ON direct_messages(timestamp);",
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

    // ─── Channels ───

    pub fn load_channels_for_space(&self, space_id: &str) -> Result<Vec<ChannelRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT id, space_id, name, room_key, channel_type, topic, voice_quality FROM channels WHERE space_id = ?1")
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
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn save_channel(&self, ch: &ChannelRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO channels (id, space_id, name, room_key, channel_type, topic, voice_quality) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![ch.id, ch.space_id, ch.name, ch.room_key, ch.channel_type, ch.topic.as_deref().unwrap_or(""), ch.voice_quality.unwrap_or(2)],
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

    pub fn set_user_status(&self, user_id: &str, status: &str) {
        if let Ok(conn) = self.lock_conn() {
            let _ = conn.execute(
                "UPDATE users SET status = ?1 WHERE user_id = ?2",
                params![status, user_id],
            );
        }
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
                        reply_to_message_id, reply_to_sender_name, reply_preview, pinned
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
                reply_to_message_id, reply_to_sender_name, reply_preview, pinned
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                msg.pinned as i64
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

    pub fn update_user_name(&self, user_id: &str, name: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE users SET display_name = ?2 WHERE user_id = ?1",
            params![user_id, name],
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
            "INSERT OR REPLACE INTO space_roles (space_id, user_id, role, assigned_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![role.space_id, role.user_id, role.role, role.assigned_at],
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
                "SELECT space_id, user_id, role, assigned_at
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
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
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
        db.save_user(&UserRow {
            user_id: "u1".into(),
            token: "tok123".into(),
            display_name: "Alice".into(),
            created_at: 1000,
            issued_at: 1000,
            last_seen_at: 1000,
        })
        .unwrap();
        let user = db.find_user_by_token("tok123").unwrap().unwrap();
        assert_eq!(user.user_id, "u1");
        assert_eq!(user.display_name, "Alice");
        assert_eq!(user.issued_at, 1000);
        assert_eq!(user.last_seen_at, 1000);

        db.rotate_user_session("u1", "tok456", "Alice 2", 2000, 3000)
            .unwrap();
        let rotated = db.find_user_by_token("tok456").unwrap().unwrap();
        assert_eq!(rotated.display_name, "Alice 2");
        assert_eq!(rotated.issued_at, 2000);
        assert_eq!(rotated.last_seen_at, 3000);

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
}
