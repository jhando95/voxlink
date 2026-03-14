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
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: String,
    pub channel_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub user_id: String,
    pub token: String,
    pub display_name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct BanRow {
    pub space_id: String,
    pub user_id: String,
    pub banned_at: i64,
}

impl Database {
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
        let conn = self.conn.lock().unwrap();
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
                FOREIGN KEY (channel_id) REFERENCES channels(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS users (
                user_id TEXT PRIMARY KEY,
                token TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS bans (
                space_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                banned_at INTEGER NOT NULL,
                PRIMARY KEY (space_id, user_id),
                FOREIGN KEY (space_id) REFERENCES spaces(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_channels_space ON channels(space_id);
            CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id);
            CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
            CREATE INDEX IF NOT EXISTS idx_users_token ON users(token);
            CREATE INDEX IF NOT EXISTS idx_bans_space ON bans(space_id);",
        )
        .map_err(|e| format!("Failed to init tables: {e}"))?;
        Ok(())
    }

    // ─── Spaces ───

    pub fn load_all_spaces(&self) -> Result<Vec<SpaceRow>, String> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO spaces (id, name, invite_code, owner_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![space.id, space.name, space.invite_code, space.owner_id, space.created_at],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn delete_space(&self, space_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM spaces WHERE id = ?1", params![space_id])
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(())
    }

    // ─── Channels ───

    pub fn load_channels_for_space(&self, space_id: &str) -> Result<Vec<ChannelRow>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, space_id, name, room_key, channel_type FROM channels WHERE space_id = ?1")
            .map_err(|e| format!("Query error: {e}"))?;
        let rows = stmt
            .query_map(params![space_id], |row| {
                Ok(ChannelRow {
                    id: row.get(0)?,
                    space_id: row.get(1)?,
                    name: row.get(2)?,
                    room_key: row.get(3)?,
                    channel_type: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row error: {e}"))
    }

    pub fn save_channel(&self, ch: &ChannelRow) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO channels (id, space_id, name, room_key, channel_type) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ch.id, ch.space_id, ch.name, ch.room_key, ch.channel_type],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    // ─── Messages ───

    pub fn load_messages_for_channel(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageRow>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, channel_id, sender_id, sender_name, content, timestamp
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (id, channel_id, sender_id, sender_name, content, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![msg.id, msg.channel_id, msg.sender_id, msg.sender_name, msg.content, msg.timestamp],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn update_message(&self, message_id: &str, new_content: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE messages SET content = ?2 WHERE id = ?1",
                params![message_id, new_content],
            )
            .map_err(|e| format!("Update error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn delete_message(&self, message_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute("DELETE FROM messages WHERE id = ?1", params![message_id])
            .map_err(|e| format!("Delete error: {e}"))?;
        Ok(rows > 0)
    }

    pub fn get_message_sender(&self, message_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT sender_id FROM messages WHERE id = ?1")
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![message_id], |row| row.get(0))
            .ok();
        Ok(result)
    }

    // ─── Users / Auth ───

    pub fn save_user(&self, user: &UserRow) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO users (user_id, token, display_name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![user.user_id, user.token, user.display_name, user.created_at],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn find_user_by_token(&self, token: &str) -> Result<Option<UserRow>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT user_id, token, display_name, created_at FROM users WHERE token = ?1")
            .map_err(|e| format!("Query error: {e}"))?;
        let result = stmt
            .query_row(params![token], |row| {
                Ok(UserRow {
                    user_id: row.get(0)?,
                    token: row.get(1)?,
                    display_name: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .ok();
        Ok(result)
    }

    pub fn update_user_name(&self, user_id: &str, name: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE users SET display_name = ?2 WHERE user_id = ?1",
            params![user_id, name],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    }

    // ─── Bans ───

    pub fn save_ban(&self, ban: &BanRow) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO bans (space_id, user_id, banned_at) VALUES (?1, ?2, ?3)",
            params![ban.space_id, ban.user_id, ban.banned_at],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        Ok(())
    }

    pub fn is_banned(&self, space_id: &str, user_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT 1 FROM bans WHERE space_id = ?1 AND user_id = ?2")
            .map_err(|e| format!("Query error: {e}"))?;
        let exists = stmt.query_row(params![space_id, user_id], |_| Ok(())).is_ok();
        Ok(exists)
    }

    pub fn load_bans_for_space(&self, space_id: &str) -> Result<Vec<BanRow>, String> {
        let conn = self.conn.lock().unwrap();
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

    /// Get the highest numeric suffix from space/channel/message IDs to restore allocators.
    pub fn max_id_suffix(&self, table: &str, col: &str) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        // table and col are controlled internally, not from user input
        let query = format!("SELECT {col} FROM {table}");
        let mut stmt = conn.prepare(&query).map_err(|e| format!("Query error: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db() -> (Database, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "voxlink_test_{}_{n}.db",
            std::process::id()
        ));
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
        })
        .unwrap();
        let channels = db.load_channels_for_space("s1").unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "General");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn message_round_trip() {
        let (db, path) = temp_db();
        // Create parent space and channel for foreign key constraints
        db.save_space(&SpaceRow {
            id: "s1".into(), name: "S".into(), invite_code: "X".into(),
            owner_id: "p1".into(), created_at: 0,
        }).unwrap();
        db.save_channel(&ChannelRow {
            id: "c1".into(), space_id: "s1".into(), name: "General".into(),
            room_key: "sp:s1:ch:c1".into(), channel_type: "text".into(),
        }).unwrap();
        db.save_message(&MessageRow {
            id: "m1".into(),
            channel_id: "c1".into(),
            sender_id: "p1".into(),
            sender_name: "Alice".into(),
            content: "Hello".into(),
            timestamp: 1000,
        })
        .unwrap();
        let msgs = db.load_messages_for_channel("c1", 50).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello");

        db.update_message("m1", "Updated").unwrap();
        let msgs = db.load_messages_for_channel("c1", 50).unwrap();
        assert_eq!(msgs[0].content, "Updated");

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
        })
        .unwrap();
        let user = db.find_user_by_token("tok123").unwrap().unwrap();
        assert_eq!(user.user_id, "u1");
        assert_eq!(user.display_name, "Alice");

        assert!(db.find_user_by_token("nonexistent").unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ban_round_trip() {
        let (db, path) = temp_db();
        // Create parent space for foreign key constraint
        db.save_space(&SpaceRow {
            id: "s1".into(), name: "S".into(), invite_code: "X".into(),
            owner_id: "p1".into(), created_at: 0,
        }).unwrap();
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
