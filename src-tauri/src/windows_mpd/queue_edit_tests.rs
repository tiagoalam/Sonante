use super::*;
struct Fake {
    live: View,
    calls: Vec<String>,
    next_id: u32,
    mutations: usize,
    fail_at: Option<usize>,
    duplicate_id: bool,
    disturb_pause: bool,
    changed_read: bool,
}
impl Fake {
    fn new(state: &str, n: usize) -> Self {
        Self {
            live: View {
                entries: (0..n)
                    .map(|i| Entry {
                        id: i as u32 + 10,
                        uri: if i < 2 {
                            "same/track.flac".into()
                        } else {
                            format!("disc/{i}.flac")
                        },
                    })
                    .collect(),
                version: 2,
                cursor: if n == 0 { None } else { Some(1.min(n - 1)) },
                state: state.into(),
                elapsed: 12.5,
            },
            calls: vec![],
            next_id: 100,
            mutations: 0,
            fail_at: None,
            duplicate_id: false,
            disturb_pause: false,
            changed_read: false,
        }
    }
    fn query(&mut self, command: &str) -> Result<Vec<String>> {
        self.calls.push(command.into());
        if command == "status" {
            let v = &self.live;
            let mut lines = vec![
                format!("state: {}", v.state),
                "volume: 37".into(),
                "duration: 120".into(),
                format!("elapsed: {}", v.elapsed),
                format!("playlist: {}", v.version),
                format!("playlistlength: {}", v.entries.len()),
            ];
            if let Some(i) = v.cursor {
                lines.extend([format!("song: {i}"), format!("songid: {}", v.entries[i].id)]);
            }
            if self.changed_read {
                self.live.version += 1;
            }
            return Ok(lines);
        }
        if let Some(range) = command.strip_prefix("playlistinfo ") {
            let (a, b) = range.split_once(':').unwrap();
            let a: usize = a.parse().unwrap();
            let b: usize = b.parse().unwrap();
            assert!(b - a <= 16);
            let mut rows = vec![];
            for pos in a..b {
                let e = &self.live.entries[pos];
                rows.extend([
                    format!("file: {}", e.uri),
                    "Artist: tags are ignored".into(),
                    format!("Pos: {pos}"),
                    format!("Id: {}", e.id),
                ]);
            }
            return Ok(rows);
        }
        let current = self.live.cursor.map(|i| self.live.entries[i].id);
        let mut response = vec![];
        if let Some(args) = command.strip_prefix("addid ") {
            let (quoted, at) = args.rsplit_once(' ').unwrap();
            let at = at.parse::<usize>().unwrap();
            let uri = ["new/á \"live\".flac", "new/b.flac", "new/c.flac"]
                .into_iter()
                .find(|u| protocol::quote(u).unwrap() == quoted)
                .expect("quoted safe URI");
            let id = self.next_id;
            self.next_id += 1;
            self.live.entries.insert(
                at,
                Entry {
                    id,
                    uri: uri.into(),
                },
            );
            response.push(format!("Id: {}", if self.duplicate_id { 10 } else { id }));
        } else if let Some(args) = command.strip_prefix("moveid ") {
            let (id, to) = args.split_once(' ').unwrap();
            let id = id.parse::<u32>().unwrap();
            let to = to.parse::<usize>().unwrap();
            let from = self.live.entries.iter().position(|e| e.id == id).unwrap();
            apply(&mut self.live.entries, Plan { from, to: Some(to) });
        } else if let Some(id) = command.strip_prefix("deleteid ") {
            let id = id.parse::<u32>().unwrap();
            let from = self.live.entries.iter().position(|e| e.id == id).unwrap();
            self.live.entries.remove(from);
        } else {
            panic!("unexpected transport or destructive replay command {command}")
        }
        self.live.cursor = current.and_then(|id| self.live.entries.iter().position(|e| e.id == id));
        self.live.version += 1;
        self.mutations += 1;
        if self.disturb_pause {
            self.live.elapsed += 1.0;
        }
        if self.fail_at == Some(self.mutations) {
            return Err("mpd_timeout");
        }
        Ok(response)
    }
    fn snapshot(&mut self) -> View {
        read(|cmd| self.query(cmd)).unwrap()
    }
}
#[test]
fn bounded_pages_preserve_duplicate_occurrences_and_validate_identity() {
    let mut f = Fake::new("pause", 35);
    let view = f.snapshot();
    assert_eq!(view.entries.len(), 35);
    assert_eq!(
        f.calls,
        [
            "status",
            "playlistinfo 0:16",
            "playlistinfo 16:32",
            "playlistinfo 32:35",
            "status"
        ]
    );
    assert_eq!(
        aligned(
            &view,
            &view.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            &view
                .entries
                .iter()
                .map(|e| e.uri.clone())
                .collect::<Vec<_>>()
        ),
        Ok(())
    );
    assert_eq!(
        aligned(&view, &[10], &["same/track.flac".into()]),
        Err("PLAYBACK_QUEUE_UNCERTAIN")
    );
    let mut wrong = view.clone();
    wrong.entries[0].id = 999;
    assert_eq!(
        aligned(
            &wrong,
            &view.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            &view
                .entries
                .iter()
                .map(|e| e.uri.clone())
                .collect::<Vec<_>>()
        ),
        Err("PLAYBACK_QUEUE_UNCERTAIN")
    );
}
#[test]
fn insertion_preserves_playing_paused_stopped_and_duplicate_ids_without_transport() {
    for state in ["play", "pause", "stop"] {
        for placement in [Placement::Next, Placement::End] {
            let mut f = Fake::new(state, 4);
            let before = f.snapshot();
            let at = position(4, before.cursor, placement).unwrap();
            let after = insert(
                &before,
                &["new/á \"live\".flac".into(), "new/b.flac".into()],
                at,
                |c| f.query(c),
            )
            .unwrap();
            assert_eq!(after.state, state);
            assert_eq!(after.elapsed, 12.5);
            assert_eq!(after.entries[after.cursor.unwrap()].id, 11);
            assert_eq!(after.entries.len(), 6);
            assert_eq!(after.entries[at].uri, "new/á \"live\".flac");
            assert_eq!(after.entries[at + 1].uri, "new/b.flac");
            assert_eq!(f.mutations, 2);
        }
    }
}
#[test]
fn insertion_before_paused_cursor_keeps_exact_occurrence() {
    let mut f = Fake::new("pause", 3);
    let before = f.snapshot();
    let after = insert(&before, &["new/b.flac".into()], 0, |c| f.query(c)).unwrap();
    assert_eq!(after.cursor, Some(2));
    assert_eq!(after.entries[2].id, 11);
}
#[test]
fn accepted_partial_timeout_does_not_retry_rollback_or_publish_success() {
    let mut f = Fake::new("play", 3);
    let before = f.snapshot();
    f.fail_at = Some(2);
    assert_eq!(
        insert(
            &before,
            &["new/b.flac".into(), "new/c.flac".into()],
            2,
            |c| f.query(c)
        )
        .unwrap_err(),
        "PLAYBACK_QUEUE_UNCERTAIN"
    );
    assert_eq!(f.live.entries.len(), 5);
    assert_eq!(before.entries.len(), 3);
    assert_eq!(f.mutations, 2);
    assert_eq!(f.calls.iter().filter(|s| s.starts_with("addid")).count(), 2);
}
#[test]
fn duplicate_add_ids_and_unproved_pause_position_are_uncertain() {
    for malformed in [true, false] {
        let mut f = Fake::new("pause", 3);
        let before = f.snapshot();
        f.duplicate_id = malformed;
        f.disturb_pause = !malformed;
        assert_eq!(
            insert(&before, &["new/b.flac".into()], 2, |c| f.query(c)).unwrap_err(),
            "PLAYBACK_QUEUE_UNCERTAIN"
        );
    }
}
#[test]
fn moves_track_exact_occurrence_with_current_cursor_and_position() {
    for state in ["play", "pause", "stop"] {
        let mut f = Fake::new(state, 4);
        let before = f.snapshot();
        let action = plan(
            4,
            before.cursor,
            state != "stop",
            Edit::Move { from: 1, to: 0 },
        )
        .unwrap();
        let after = edit(&before, action, |c| f.query(c)).unwrap();
        assert_eq!(after.entries[0].id, 11);
        assert_eq!(after.cursor, Some(0));
        assert_eq!(after.state, state);
        assert_eq!(after.elapsed, 12.5);
        assert_eq!(f.mutations, 1);
    }
}
#[test]
fn removes_only_selected_noncurrent_duplicate_without_disturbing_pause() {
    let mut f = Fake::new("pause", 4);
    let before = f.snapshot();
    let action = plan(4, before.cursor, true, Edit::Remove { index: 0 }).unwrap();
    let after = edit(&before, action, |c| f.query(c)).unwrap();
    assert_eq!(after.cursor, Some(0));
    assert_eq!(after.entries[0].id, 11);
    assert_eq!(after.elapsed, 12.5);
    assert_eq!(
        plan(4, Some(1), true, Edit::Remove { index: 1 }),
        Err("QUEUE_EDIT_CURRENT")
    );
}
#[test]
fn play_next_is_a_move_after_current_even_from_before_current() {
    let p = plan(5, Some(3), true, Edit::Next { index: 1 }).unwrap();
    assert_eq!(
        p,
        Plan {
            from: 1,
            to: Some(3)
        }
    );
    assert_eq!(remap(5, 3, p), Some(2));
    let mut rows = vec![0, 1, 2, 3, 4];
    apply(&mut rows, p);
    assert_eq!(rows, [0, 2, 3, 1, 4]);
    assert_eq!(
        plan(5, Some(3), true, Edit::Next { index: 3 }),
        Ok(Plan {
            from: 3,
            to: Some(3)
        })
    );
}
#[test]
fn no_op_never_sends_a_mutation_and_invalid_indices_fail_before_io() {
    let mut f = Fake::new("pause", 3);
    let before = f.snapshot();
    let n = f.calls.len();
    edit(
        &before,
        Plan {
            from: 1,
            to: Some(1),
        },
        |c| f.query(c),
    )
    .unwrap();
    assert_eq!(f.calls.len(), n);
    for action in [
        Edit::Move { from: 3, to: 0 },
        Edit::Move { from: 0, to: 3 },
        Edit::Remove { index: 3 },
        Edit::Next { index: 3 },
    ] {
        assert_eq!(plan(3, Some(1), true, action), Err("QUEUE_EDIT_CHANGED"));
    }
}
#[test]
fn delete_failure_keeps_prior_projection_and_has_no_replay() {
    let mut f = Fake::new("pause", 3);
    let before = f.snapshot();
    f.fail_at = Some(1);
    assert_eq!(
        edit(&before, Plan { from: 0, to: None }, |c| f.query(c)).unwrap_err(),
        "PLAYBACK_QUEUE_UNCERTAIN"
    );
    assert_eq!(f.mutations, 1);
    assert_eq!(before.entries.len(), 3);
    assert_eq!(f.live.entries.len(), 2);
}
#[test]
fn stopped_last_removal_can_prove_an_empty_queue_without_play() {
    let mut f = Fake::new("stop", 1);
    let before = f.snapshot();
    let after = edit(&before, Plan { from: 0, to: None }, |c| f.query(c)).unwrap();
    assert!(after.entries.is_empty());
    assert_eq!(after.cursor, None);
    assert_eq!(after.state, "stop");
    assert_eq!(remap(1, 0, Plan { from: 0, to: None }), None);
}
#[test]
fn unsafe_uri_is_rejected_before_the_first_mutation() {
    let mut f = Fake::new("play", 3);
    let before = f.snapshot();
    let n = f.calls.len();
    assert_eq!(
        insert(&before, &["bad\nclear".into()], 0, |c| f.query(c)).unwrap_err(),
        "invalid_argument"
    );
    assert_eq!(f.calls.len(), n);
}
#[test]
fn inconsistent_snapshot_and_duplicate_status_are_rejected() {
    let mut f = Fake::new("pause", 3);
    f.changed_read = true;
    assert_eq!(read(|c| f.query(c)).unwrap_err(), "QUEUE_EDIT_CHANGED");
    assert_eq!(
        version(&["playlist: 1".into(), "playlist: 2".into()]),
        Err("mpd_protocol")
    );
    assert!(entries(&["file: x".into(), "Id: 4".into(), "Pos: 1".into()], 0, 1).is_err());
    let mut f = Fake::new("pause", 2);
    f.live.entries[1].id = 10;
    assert!(read(|c| f.query(c)).is_err());
}
