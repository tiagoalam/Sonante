//! Personal collection references. References are opaque and are not resolved here.
use super::{Catalog, Connection};
use rusqlite::{params, OptionalExtension};

pub(crate) const SCHEMA: &str = "CREATE TABLE personal_collections(\
 id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 256),\
 created_ns INTEGER NOT NULL CHECK(created_ns>=0), updated_ns INTEGER NOT NULL CHECK(updated_ns>=0));\
 CREATE TABLE personal_collection_members(\
 id INTEGER PRIMARY KEY AUTOINCREMENT, collection_id INTEGER NOT NULL REFERENCES personal_collections(id) ON DELETE CASCADE,\
 source TEXT NOT NULL CHECK(source IN ('local','plex','subsonic')), scope TEXT NOT NULL CHECK(length(scope) BETWEEN 1 AND 1024),\
 reference TEXT NOT NULL CHECK(length(reference) BETWEEN 1 AND 4096), title TEXT NOT NULL CHECK(length(trim(title)) BETWEEN 1 AND 1024),\
 artist TEXT, position INTEGER NOT NULL CHECK(position>=0),\
 UNIQUE(collection_id,source,scope,reference), UNIQUE(collection_id,position));";

// Paths are raw relative path bytes, preserving non-UTF-8 Linux names. The future
// capture layer must validate that each path stays within its authorized root.
// Unsigned CUE fields use eight-byte big-endian blobs to preserve the full u64 range.
pub(crate) const EVIDENCE_SCHEMA: &str = "
 CREATE TABLE collection_local_snapshots(
 member_id INTEGER PRIMARY KEY REFERENCES personal_collection_members(id) ON DELETE CASCADE,
 version INTEGER NOT NULL CHECK(version=2), album_locator TEXT NOT NULL CHECK(length(album_locator) BETWEEN 1 AND 16384));
 CREATE TABLE collection_local_files(
 member_id INTEGER NOT NULL REFERENCES collection_local_snapshots(member_id) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position>=0), path BLOB NOT NULL CHECK(typeof(path)='blob' AND length(path) BETWEEN 1 AND 16384),
 size INTEGER NOT NULL CHECK(size>=0), mtime_ns INTEGER NOT NULL,
 sample_sha256 BLOB NOT NULL CHECK(typeof(sample_sha256)='blob' AND length(sample_sha256)=32),
 PRIMARY KEY(member_id,position), UNIQUE(member_id,path));
 CREATE TABLE collection_cue_segments(
 member_id INTEGER NOT NULL REFERENCES collection_local_snapshots(member_id) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position>=0), cue_path BLOB NOT NULL CHECK(typeof(cue_path)='blob' AND length(cue_path) BETWEEN 1 AND 16384),
 audio_path BLOB NOT NULL CHECK(typeof(audio_path)='blob' AND length(audio_path) BETWEEN 1 AND 16384),
 track INTEGER NOT NULL CHECK(track>0), start_frame BLOB NOT NULL CHECK(typeof(start_frame)='blob' AND length(start_frame)=8),
 end_frame BLOB CHECK(end_frame IS NULL OR (typeof(end_frame)='blob' AND length(end_frame)=8)),
 cue_size INTEGER NOT NULL CHECK(cue_size>=0), cue_mtime_ns INTEGER NOT NULL,
 cue_device BLOB NOT NULL CHECK(typeof(cue_device)='blob' AND length(cue_device)=8),
 cue_inode BLOB NOT NULL CHECK(typeof(cue_inode)='blob' AND length(cue_inode)=8), cue_ctime_ns INTEGER NOT NULL,
 PRIMARY KEY(member_id,position), FOREIGN KEY(member_id,audio_path) REFERENCES collection_local_files(member_id,path) ON DELETE CASCADE);
";

pub(crate) fn validate_evidence_schema(db: &Connection) -> Result<(), String> {
    for (table, expected) in [("collection_local_snapshots", 1), ("collection_local_files", 1), ("collection_cue_segments", 3)] {
        let count: i64 = db.query_row(&format!("SELECT count(*) FROM pragma_foreign_key_list('{table}') WHERE on_delete='CASCADE'"), [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if count != expected { return Err(format!("invalid {table} foreign keys")); }
    }
    Ok(())
}

pub(crate) fn validate_schema(db: &Connection) -> Result<(), String> {
    super::validate_table_columns(
        db,
        "personal_collections",
        &[
            ("id", "INTEGER", 0, 1),
            ("name", "TEXT", 1, 0),
            ("created_ns", "INTEGER", 1, 0),
            ("updated_ns", "INTEGER", 1, 0),
        ],
    )?;
    super::validate_table_columns(
        db,
        "personal_collection_members",
        &[
            ("id", "INTEGER", 0, 1),
            ("collection_id", "INTEGER", 1, 0),
            ("source", "TEXT", 1, 0),
            ("scope", "TEXT", 1, 0),
            ("reference", "TEXT", 1, 0),
            ("title", "TEXT", 1, 0),
            ("artist", "TEXT", 0, 0),
            ("position", "INTEGER", 1, 0),
        ],
    )?;
    let fk: i64 = db.query_row("SELECT count(*) FROM pragma_foreign_key_list('personal_collection_members') WHERE \"table\"='personal_collections' AND \"from\"='collection_id' AND on_delete='CASCADE'", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let indexes: i64 = db.query_row("SELECT count(*) FROM pragma_index_list('personal_collection_members') WHERE \"unique\"=1", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let index_columns: i64 = db.query_row("SELECT count(*) FROM pragma_index_list('personal_collection_members') AS i WHERE i.\"unique\"=1 AND (SELECT group_concat(name, ',') FROM pragma_index_info(i.name)) IN ('collection_id,source,scope,reference','collection_id,position')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if fk != 1 || indexes != 2 || index_columns != 2 {
        return Err("invalid collections constraints".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub created_ns: i64,
    pub updated_ns: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub id: i64,
    pub collection_id: i64,
    pub source: String,
    pub scope: String,
    pub reference: String,
    pub title: String,
    pub artist: Option<String>,
    pub position: i64,
}
#[derive(Debug, Clone)]
pub struct NewMember {
    pub source: String,
    pub scope: String,
    pub reference: String,
    pub title: String,
    pub artist: Option<String>,
}
// These values are evidence, not proof of full file contents. Paths are raw
// relative bytes; the source-specific caller must validate them against its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEvidenceFile {
    pub path: Vec<u8>, pub size: i64, pub mtime_ns: i64, pub sample_sha256: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEvidenceSegment {
    pub cue_path: Vec<u8>, pub audio_path: Vec<u8>, pub track: u32,
    pub start_frame: u64, pub end_frame: Option<u64>, pub cue_size: i64,
    pub cue_mtime_ns: i64, pub cue_device: u64, pub cue_inode: u64, pub cue_ctime_ns: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEvidence {
    pub album_locator: String, pub files: Vec<LocalEvidenceFile>, pub segments: Vec<LocalEvidenceSegment>,
}
// Prevent unbounded allocations on a damaged database, while allowing very large albums.
const MAX_EVIDENCE_ROWS: usize = 100_000;
const MAX_EVIDENCE_PATH_BYTES: usize = 64 * 1024 * 1024;
fn parse_u64_blob(value: Vec<u8>) -> rusqlite::Result<u64> {
    let bytes: [u8; 8] = value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(u64::from_be_bytes(bytes))
}

fn field(value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err("COLLECTION_INVALID_INPUT".into());
    }
    Ok(())
}
fn identity(value: &str, max: usize) -> Result<(), String> {
    field(value, max)?;
    let lower = value.to_ascii_lowercase();
    if lower.contains("://")
        || lower.contains('?')
        || lower.contains('#')
        || lower.contains('@')
        || lower.contains("token=")
        || lower.contains("password=")
        || lower.contains("apikey=")
    {
        return Err("COLLECTION_INVALID_REFERENCE".into());
    }
    Ok(())
}
fn exists(db: &Connection, id: i64) -> Result<(), String> {
    let present: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM personal_collections WHERE id=?)",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !present {
        return Err("COLLECTION_NOT_FOUND".into());
    }
    Ok(())
}
impl Catalog {
    pub fn collection_create(&mut self, name: &str) -> Result<i64, String> {
        field(name, 256)?;
        let now = super::now_ns();
        self.db
            .connection
            .execute(
                "INSERT INTO personal_collections(name,created_ns,updated_ns) VALUES(?,?,?)",
                params![name.trim(), now, now],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.db.connection.last_insert_rowid())
    }
    pub fn collection_rename(&mut self, id: i64, name: &str) -> Result<(), String> {
        field(name, 256)?;
        let n = self
            .db
            .connection
            .execute(
                "UPDATE personal_collections SET name=?,updated_ns=? WHERE id=?",
                params![name.trim(), super::now_ns(), id],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("COLLECTION_NOT_FOUND".into());
        }
        Ok(())
    }
    pub fn collection_delete(&mut self, id: i64) -> Result<(), String> {
        let n = self
            .db
            .connection
            .execute("DELETE FROM personal_collections WHERE id=?", [id])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("COLLECTION_NOT_FOUND".into());
        }
        Ok(())
    }
    pub fn collections(&self) -> Result<Vec<Collection>, String> {
        let mut q = self
            .db
            .connection
            .prepare("SELECT id,name,created_ns,updated_ns FROM personal_collections ORDER BY id")
            .map_err(|e| e.to_string())?;
        let result = q
            .query_map([], |r| {
                Ok(Collection {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_ns: r.get(2)?,
                    updated_ns: r.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string());
        result
    }
    pub fn collection_add(&mut self, id: i64, item: NewMember) -> Result<i64, String> {
        self.collection_add_with_evidence(id, item, None, None)
    }
    pub fn collection_add_local_evidence(&mut self, id: i64, item: NewMember, evidence: &LocalEvidence, legacy_reference: Option<&str>) -> Result<i64, String> {
        if item.source != "local" || !item.reference.starts_with("local2:") || evidence.files.is_empty()
            || evidence.files.len() > MAX_EVIDENCE_ROWS || evidence.segments.len() > MAX_EVIDENCE_ROWS {
            return Err("COLLECTION_LOCAL_INVALID".into());
        }
        let path_bytes = evidence.files.iter().map(|f| f.path.len()).chain(
            evidence.segments.iter().flat_map(|s| [s.cue_path.len(), s.audio_path.len()])
        ).try_fold(0usize, |sum, len| sum.checked_add(len)).ok_or("COLLECTION_LOCAL_INVALID")?;
        if path_bytes > MAX_EVIDENCE_PATH_BYTES { return Err("COLLECTION_LOCAL_INVALID".into()); }
        self.collection_add_with_evidence(id, item, Some(evidence), legacy_reference)
    }
    fn collection_add_with_evidence(&mut self, id: i64, item: NewMember, evidence: Option<&LocalEvidence>, legacy_reference: Option<&str>) -> Result<i64, String> {
        if !matches!(item.source.as_str(), "local" | "plex" | "subsonic") {
            return Err("COLLECTION_INVALID_SOURCE".into());
        }
        identity(&item.scope, 1024)?;
        identity(&item.reference, 4096)?;
        field(&item.title, 1024)?;
        if let Some(artist) = &item.artist {
            field(artist, 1024)?;
        }
        let tx = self
            .db
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        exists(&tx, id)?;
        let duplicate: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM personal_collection_members WHERE collection_id=? AND source=? AND scope=? AND reference=?)",params![id,item.source,item.scope,item.reference],|r|r.get(0)).map_err(|e|e.to_string())?;
        let legacy_duplicate: bool = if let Some(legacy) = legacy_reference {
            identity(legacy, 4096)?;
            tx.query_row("SELECT EXISTS(SELECT 1 FROM personal_collection_members WHERE collection_id=?1 AND source='local' AND scope=?2 AND reference=?3)", params![id, item.scope, legacy], |r| r.get(0)).map_err(|e| e.to_string())?
        } else { false };
        if duplicate || legacy_duplicate {
            return Err("COLLECTION_DUPLICATE_MEMBER".into());
        }
        let position:i64=tx.query_row("SELECT COALESCE(MAX(position)+1,0) FROM personal_collection_members WHERE collection_id=?",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO personal_collection_members(collection_id,source,scope,reference,title,artist,position) VALUES(?,?,?,?,?,?,?)",params![id,item.source,item.scope,item.reference,item.title,item.artist,position]).map_err(|e|e.to_string())?;
        let member = tx.last_insert_rowid();
        if let Some(evidence) = evidence {
            tx.execute("INSERT INTO collection_local_snapshots(member_id,version,album_locator) VALUES(?1,2,?2)", params![member, evidence.album_locator]).map_err(|e| e.to_string())?;
            for (position, f) in evidence.files.iter().enumerate() {
                tx.execute("INSERT INTO collection_local_files(member_id,position,path,size,mtime_ns,sample_sha256) VALUES(?1,?2,?3,?4,?5,?6)", params![member, position as i64, f.path, f.size, f.mtime_ns, f.sample_sha256]).map_err(|e| e.to_string())?;
            }
            for (position, c) in evidence.segments.iter().enumerate() {
                tx.execute("INSERT INTO collection_cue_segments(member_id,position,cue_path,audio_path,track,start_frame,end_frame,cue_size,cue_mtime_ns,cue_device,cue_inode,cue_ctime_ns) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![member, position as i64, c.cue_path, c.audio_path, c.track, c.start_frame.to_be_bytes().as_slice(), c.end_frame.map(|v| v.to_be_bytes().to_vec()), c.cue_size, c.cue_mtime_ns, c.cue_device.to_be_bytes().as_slice(), c.cue_inode.to_be_bytes().as_slice(), c.cue_ctime_ns]).map_err(|e| e.to_string())?;
            }
        }
        tx.execute(
            "UPDATE personal_collections SET updated_ns=? WHERE id=?",
            params![super::now_ns(), id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(member)
    }
    pub fn collection_local_evidence(&mut self, collection: i64, member: i64) -> Result<LocalEvidence, String> {
        let tx = self.db.connection.transaction().map_err(|e| e.to_string())?;
        let locator: String = tx.query_row("SELECT s.album_locator FROM collection_local_snapshots s JOIN personal_collection_members m ON m.id=s.member_id WHERE m.id=?1 AND m.collection_id=?2 AND m.source='local' AND s.version=2", params![member, collection], |r| r.get(0)).map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        let mut path_bytes = 0usize;
        {
            let mut q = tx.prepare("SELECT path,size,mtime_ns,sample_sha256 FROM collection_local_files WHERE member_id=?1 ORDER BY position LIMIT 100001").map_err(|e| e.to_string())?;
            let rows = q.query_map([member], |r| Ok(LocalEvidenceFile { path:r.get(0)?,size:r.get(1)?,mtime_ns:r.get(2)?,sample_sha256:r.get(3)? })).map_err(|e| e.to_string())?;
            for row in rows {
                let file = row.map_err(|e| e.to_string())?;
                path_bytes = path_bytes.checked_add(file.path.len()).ok_or("COLLECTION_LOCAL_INVALID")?;
                if path_bytes > MAX_EVIDENCE_PATH_BYTES { return Err("COLLECTION_LOCAL_INVALID".into()); }
                files.push(file);
            }
        }
        let mut segments = Vec::new();
        {
            let mut q = tx.prepare("SELECT cue_path,audio_path,track,start_frame,end_frame,cue_size,cue_mtime_ns,cue_device,cue_inode,cue_ctime_ns FROM collection_cue_segments WHERE member_id=?1 ORDER BY position LIMIT 100001").map_err(|e| e.to_string())?;
            let rows = q.query_map([member], |r| Ok(LocalEvidenceSegment { cue_path:r.get(0)?,audio_path:r.get(1)?,track:r.get(2)?,start_frame:parse_u64_blob(r.get(3)?)?,end_frame:r.get::<_,Option<Vec<u8>>>(4)?.map(parse_u64_blob).transpose()?,cue_size:r.get(5)?,cue_mtime_ns:r.get(6)?,cue_device:parse_u64_blob(r.get(7)?)?,cue_inode:parse_u64_blob(r.get(8)?)?,cue_ctime_ns:r.get(9)? })).map_err(|e| e.to_string())?;
            for row in rows {
                let segment = row.map_err(|e| e.to_string())?;
                path_bytes = path_bytes.checked_add(segment.cue_path.len()).and_then(|n| n.checked_add(segment.audio_path.len())).ok_or("COLLECTION_LOCAL_INVALID")?;
                if path_bytes > MAX_EVIDENCE_PATH_BYTES { return Err("COLLECTION_LOCAL_INVALID".into()); }
                segments.push(segment);
            }
        }
        if files.is_empty() || files.len() > MAX_EVIDENCE_ROWS || segments.len() > MAX_EVIDENCE_ROWS { return Err("COLLECTION_LOCAL_INVALID".into()); }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(LocalEvidence { album_locator: locator, files, segments })
    }
    pub fn collection_remove(&mut self, id: i64, member: i64) -> Result<(), String> {
        let tx = self
            .db
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        exists(&tx, id)?;
        let n = tx
            .execute(
                "DELETE FROM personal_collection_members WHERE collection_id=? AND id=?",
                params![id, member],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("COLLECTION_MEMBER_NOT_FOUND".into());
        }
        tx.execute(
            "UPDATE personal_collections SET updated_ns=? WHERE id=?",
            params![super::now_ns(), id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn collection_member(&self, id: i64, member_id: i64) -> Result<Member, String> {
        exists(&self.db.connection, id)?;
        self.db.connection.query_row(
            "SELECT id,collection_id,source,scope,reference,title,artist,position FROM personal_collection_members WHERE id=? AND collection_id=?",
            params![member_id,id],
            |r| Ok(Member { id:r.get(0)?, collection_id:r.get(1)?, source:r.get(2)?, scope:r.get(3)?, reference:r.get(4)?, title:r.get(5)?, artist:r.get(6)?, position:r.get(7)? }),
        ).optional().map_err(|e|e.to_string())?.ok_or_else(|| "COLLECTION_MEMBER_NOT_FOUND".into())
    }
    pub fn collection_members(&self, id: i64) -> Result<Vec<Member>, String> {
        exists(&self.db.connection, id)?;
        let mut q=self.db.connection.prepare("SELECT id,collection_id,source,scope,reference,title,artist,position FROM personal_collection_members WHERE collection_id=? ORDER BY position,id").map_err(|e|e.to_string())?;
        let result = q
            .query_map([id], |r| {
                Ok(Member {
                    id: r.get(0)?,
                    collection_id: r.get(1)?,
                    source: r.get(2)?,
                    scope: r.get(3)?,
                    reference: r.get(4)?,
                    title: r.get(5)?,
                    artist: r.get(6)?,
                    position: r.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string());
        result
    }
}
