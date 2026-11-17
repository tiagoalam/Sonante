//! Native MPD queue mutations. Read/confirm exact occurrence IDs; never replay on uncertainty.
use super::protocol::{self, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::{Duration, Instant};
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Next,
    End,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Edit {
    Move { from: usize, to: usize },
    Next { index: usize },
    Remove { index: usize },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u32,
    pub uri: String,
}
#[derive(Clone, Debug)]
pub struct View {
    pub entries: Vec<Entry>,
    pub version: u64,
    pub cursor: Option<usize>,
    pub state: String,
    pub elapsed: f64,
}
pub fn version(lines: &[String]) -> Result<u64> {
    protocol::field(lines, "playlist")?
        .ok_or("mpd_protocol")?
        .parse()
        .map_err(|_| "mpd_protocol")
}
pub fn revision(generation: u64, version: Option<u64>) -> String {
    format!(
        "wq1:{generation}:{}",
        version.map_or_else(|| "logical".into(), |v| v.to_string())
    )
}
fn length(lines: &[String]) -> Result<usize> {
    let n = protocol::field(lines, "playlistlength")?
        .ok_or("mpd_protocol")?
        .parse()
        .map_err(|_| "mpd_protocol")?;
    if n > 1000 {
        return Err("queue_limit");
    }
    Ok(n)
}
fn entries(lines: &[String], start: usize, count: usize) -> Result<Vec<Entry>> {
    let mut result = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let flush = |row: &mut Vec<String>, result: &mut Vec<Entry>| -> Result<()> {
        if row.is_empty() {
            return Ok(());
        }
        let uri = protocol::field(row, "file")?.ok_or("mpd_protocol")?;
        protocol::quote(uri)?;
        let id = protocol::field(row, "Id")?
            .ok_or("mpd_protocol")?
            .parse()
            .map_err(|_| "mpd_protocol")?;
        let pos: usize = protocol::field(row, "Pos")?
            .ok_or("mpd_protocol")?
            .parse()
            .map_err(|_| "mpd_protocol")?;
        if pos != start + result.len() {
            return Err("mpd_protocol");
        }
        result.push(Entry {
            id,
            uri: uri.into(),
        });
        row.clear();
        Ok(())
    };
    for line in lines {
        if line.starts_with("file: ") {
            flush(&mut row, &mut result)?;
        }
        if row.is_empty() && !line.starts_with("file: ") {
            return Err("mpd_protocol");
        }
        row.push(line.clone());
    }
    flush(&mut row, &mut result)?;
    if result.len() != count {
        return Err("mpd_protocol");
    }
    Ok(result)
}
pub fn read(mut query: impl FnMut(&str) -> Result<Vec<String>>) -> Result<View> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let before = query("status")?;
    let v = version(&before)?;
    let n = length(&before)?;
    let mut all = Vec::with_capacity(n);
    for start in (0..n).step_by(16) {
        if Instant::now() >= deadline {
            return Err("mpd_timeout");
        }
        let end = (start + 16).min(n);
        all.extend(entries(
            &query(&format!("playlistinfo {start}:{end}"))?,
            start,
            end - start,
        )?);
    }
    let after = query("status")?;
    if version(&after)? != v || length(&after)? != n {
        return Err("QUEUE_EDIT_CHANGED");
    }
    let state = protocol::playback(&after)?;
    if all.iter().map(|e| e.id).collect::<HashSet<_>>().len() != n {
        return Err("mpd_protocol");
    }
    let cursor = match (
        protocol::field(&after, "song")?,
        protocol::field(&after, "songid")?,
    ) {
        (Some(pos), Some(id)) => {
            let pos: usize = pos.parse().map_err(|_| "mpd_protocol")?;
            let id: u32 = id.parse().map_err(|_| "mpd_protocol")?;
            if all.get(pos).map(|e| e.id) != Some(id) {
                return Err("mpd_protocol");
            }
            Some(pos)
        }
        (None, None) if state.state == "stop" => None,
        _ => return Err("mpd_protocol"),
    };
    Ok(View {
        entries: all,
        version: v,
        cursor,
        state: state.state,
        elapsed: state.elapsed,
    })
}
pub fn aligned(view: &View, ids: &[u32], uris: &[String]) -> Result<()> {
    if view.entries.len() != ids.len()
        || ids.len() != uris.len()
        || view
            .entries
            .iter()
            .zip(ids.iter().zip(uris))
            .any(|(e, (id, uri))| e.id != *id || e.uri != *uri)
    {
        return Err("PLAYBACK_QUEUE_UNCERTAIN");
    }
    Ok(())
}
pub fn position(n: usize, cursor: Option<usize>, placement: Placement) -> Result<usize> {
    if cursor.is_some_and(|i| i >= n) {
        return Err("QUEUE_EDIT_CHANGED");
    }
    Ok(match placement {
        Placement::End => n,
        Placement::Next => cursor.map_or(0, |i| i + 1),
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub from: usize,
    pub to: Option<usize>,
}
pub fn plan(n: usize, cursor: Option<usize>, active: bool, action: Edit) -> Result<Plan> {
    let (from, to) = match action {
        Edit::Move { from, to } => (from, Some(to)),
        Edit::Next { index } => (
            index,
            Some(match cursor {
                Some(i) if i == index => index,
                Some(i) if index < i => i,
                Some(i) => i + 1,
                None => 0,
            }),
        ),
        Edit::Remove { index } => (index, None),
    };
    if from >= n || to.is_some_and(|i| i >= n) || cursor.is_some_and(|i| i >= n) {
        return Err("QUEUE_EDIT_CHANGED");
    }
    if to.is_none() && active && cursor == Some(from) {
        return Err("QUEUE_EDIT_CURRENT");
    }
    Ok(Plan { from, to })
}
pub fn apply<T>(rows: &mut Vec<T>, plan: Plan) {
    let value = rows.remove(plan.from);
    if let Some(to) = plan.to {
        rows.insert(to, value);
    }
}
pub fn remap(n: usize, cursor: usize, plan: Plan) -> Option<usize> {
    let mut order: Vec<_> = (0..n).collect();
    apply(&mut order, plan);
    if order.is_empty() {
        None
    } else {
        Some(
            order
                .iter()
                .position(|i| *i == cursor)
                .unwrap_or(cursor.min(order.len() - 1)),
        )
    }
}
fn confirm(before: &View, expected: &[Entry], after: &View) -> Result<()> {
    if after.entries != expected || (before.state != "play" && before.state != after.state) {
        return Err("PLAYBACK_QUEUE_UNCERTAIN");
    }
    if before.state != "play" {
        if let Some(old) = before
            .cursor
            .and_then(|i| before.entries.get(i))
            .filter(|old| expected.iter().any(|e| e.id == old.id))
        {
            let current = after.cursor.and_then(|i| after.entries.get(i));
            if current.map(|e| e.id) != Some(old.id)
                || (after.elapsed - before.elapsed).abs() > 0.25
            {
                return Err("PLAYBACK_QUEUE_UNCERTAIN");
            }
        }
    }
    Ok(())
}
pub fn insert(
    before: &View,
    uris: &[String],
    at: usize,
    mut query: impl FnMut(&str) -> Result<Vec<String>>,
) -> Result<View> {
    if uris.is_empty() || before.entries.len() + uris.len() > 1000 || at > before.entries.len() {
        return Err("queue_limit");
    }
    let quoted = uris
        .iter()
        .map(|u| protocol::quote(u))
        .collect::<Result<Vec<_>>>()?;
    // The first successful add is the linearization point. Finish this bounded
    // request, then honor later intents; never stop halfway for cancellation.
    let result: Result<View> = (|| {
        let mut expected = before.entries.clone();
        let mut ids: HashSet<_> = expected.iter().map(|e| e.id).collect();
        let deadline = Instant::now() + Duration::from_secs(20);
        for (offset, uri) in quoted.iter().enumerate() {
            if Instant::now() >= deadline {
                return Err("mpd_timeout");
            }
            let lines = query(&format!("addid {uri} {}", at + offset))?;
            let id = protocol::field(&lines, "Id")?
                .ok_or("mpd_protocol")?
                .parse::<u32>()
                .map_err(|_| "mpd_protocol")?;
            if lines.len() != 1 || !ids.insert(id) {
                return Err("mpd_protocol");
            }
            expected.insert(
                at + offset,
                Entry {
                    id,
                    uri: uris[offset].clone(),
                },
            );
        }
        let after = read(&mut query)?;
        confirm(before, &expected, &after)?;
        Ok(after)
    })();
    result.map_err(|_| "PLAYBACK_QUEUE_UNCERTAIN")
}
pub fn edit(
    before: &View,
    plan: Plan,
    mut query: impl FnMut(&str) -> Result<Vec<String>>,
) -> Result<View> {
    if plan.to == Some(plan.from) {
        return Ok(before.clone());
    }
    let selected = before.entries.get(plan.from).ok_or("QUEUE_EDIT_CHANGED")?;
    let mut expected = before.entries.clone();
    apply(&mut expected, plan);
    let command = plan.to.map_or_else(
        || format!("deleteid {}", selected.id),
        |to| format!("moveid {} {to}", selected.id),
    );
    let result: Result<View> = (|| {
        query(&command)?;
        let after = read(&mut query)?;
        confirm(before, &expected, &after)?;
        Ok(after)
    })();
    result.map_err(|_| "PLAYBACK_QUEUE_UNCERTAIN")
}
#[cfg(test)]
#[path = "queue_edit_tests.rs"]
mod tests;
