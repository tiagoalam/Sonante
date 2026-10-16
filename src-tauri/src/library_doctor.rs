//! Read-only observations of tags in the current canonical album membership.
//! Numeric accessors cannot distinguish missing tags from unparseable values.
//! Dates are interpreted only as valid YYYY, YYYY-MM or YYYY-MM-DD; originals
//! are retained. This is neither a filesystem inventory nor a CUE validator.

use crate::audio::{AudioEngine, LocalAlbumDiscTracks};
use crate::local_media::is_virtual_cue_track_uri;
use crate::tags::{
    self, LocalAlbumTrackInspection, LocalAlbumTrackKind, LocalTrackTags, LocalTrackTagsError,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorError {
    StateUnavailable,
    CatalogReadFailed,
    AlbumNotFound,
    TaskFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCode {
    AlbumArtistAbsent,
    AlbumArtistEmpty,
    DateUninterpretable,
    YearDivergence,
    NumberIndeterminate,
    TrackExceedsTotal,
    DiscExceedsTotal,
    PossibleNumberConflict,
    CueChecksNotPerformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingLevel {
    Advisory,
    Inconsistency,
    PossibleConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    AlbumArtist,
    Date,
    TrackNumber,
    TrackTotal,
    DiscNumber,
    DiscTotal,
    Cue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Evidence {
    pub uri: String,
    pub field: Field,
    /// Original text or the number obtained by the existing tag accessor.
    /// None for a number is indeterminate, not proof of a missing tag.
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: FindingCode,
    pub level: FindingLevel,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadFailure {
    pub uri: String,
    pub error: LocalTrackTagsError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnperformedCheck {
    MissingTracks,
    ContentDuplicates,
    CanonicalizationDiscardedEntries,
    CueMetadata,
    CueNumbering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub canonical_track_count: usize,
    pub physical_track_count: usize,
    pub physical_tracks_read: usize,
    pub failed_physical_track_count: usize,
    pub cue_virtual_count: usize,
    /// Successful physical tag reads only; says nothing about CUE coverage.
    pub physical_inspection_complete: bool,
    pub partial: bool,
    pub checks_not_performed: Vec<UnperformedCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub album_id: String,
    /// Includes all obtained values, even when they produce no finding.
    pub tracks: Vec<LocalAlbumTrackInspection>,
    pub coverage: Coverage,
    pub findings: Vec<Finding>,
    pub failures: Vec<ReadFailure>,
}

pub(crate) fn inspect_album_at(
    socket_path: &str,
    library_dir: &Path,
    album_id: &str,
    local_folders: &[String],
) -> Result<Report, DoctorError> {
    inspect_album_with(
        album_id,
        || AudioEngine::get_local_album_tracks_at(socket_path, album_id),
        |uri| tags::inspect_local_track_tags(library_dir, uri, local_folders),
    )
}

fn inspect_album_with(
    album_id: &str,
    resolve: impl FnOnce() -> Result<Option<Vec<LocalAlbumDiscTracks>>, String>,
    mut inspect: impl FnMut(&str) -> Result<LocalTrackTags, LocalTrackTagsError>,
) -> Result<Report, DoctorError> {
    // The resolver retains the existing catalog/update guards. Never use disc
    // labels/numbers from the catalog as evidence of a physically declared disc.
    let groups = resolve()
        .map_err(|_| DoctorError::CatalogReadFailed)?
        .ok_or(DoctorError::AlbumNotFound)?;
    let mut tracks = Vec::new();
    for track in groups.into_iter().flat_map(|group| group.tracks) {
        let uri = track.path;
        let kind = if is_virtual_cue_track_uri(&uri) {
            LocalAlbumTrackKind::CueVirtual
        } else {
            LocalAlbumTrackKind::Physical
        };
        let (tags, issue) = if kind == LocalAlbumTrackKind::CueVirtual {
            (None, None) // CUE is explicitly outside this round's checks.
        } else {
            match inspect(&uri) {
                Ok(tags) => (Some(tags), None),
                Err(error) => (None, Some(error)),
            }
        };
        tracks.push(LocalAlbumTrackInspection {
            uri,
            kind,
            tags,
            issue,
        });
    }
    if tracks.is_empty() {
        return Err(DoctorError::AlbumNotFound);
    }
    Ok(diagnose(album_id, tracks))
}

fn evidence(uri: &str, field: Field, value: Option<String>) -> Evidence {
    Evidence {
        uri: uri.to_string(),
        field,
        value,
    }
}

fn finding(code: FindingCode, level: FindingLevel, evidence: Vec<Evidence>) -> Finding {
    Finding {
        code,
        level,
        evidence,
    }
}

fn diagnose(album_id: &str, tracks: Vec<LocalAlbumTrackInspection>) -> Report {
    let mut findings = Vec::new();
    let mut failures = Vec::new();
    let mut physical_tracks_read = 0;
    let mut cue_virtual_count = 0;
    let mut years = BTreeMap::<u32, Vec<Evidence>>::new();
    let mut numbers = BTreeMap::<(u32, u32), Vec<Evidence>>::new();
    for track in &tracks {
        let uri = &track.uri;
        if track.kind == LocalAlbumTrackKind::CueVirtual {
            cue_virtual_count += 1;
            findings.push(finding(
                FindingCode::CueChecksNotPerformed,
                FindingLevel::Advisory,
                vec![evidence(uri, Field::Cue, Some("present".to_string()))],
            ));
            continue;
        }
        let Some(tags) = track.tags.as_ref().filter(|tags| tags.readable) else {
            failures.push(ReadFailure {
                uri: uri.clone(),
                error: track
                    .issue
                    .clone()
                    .unwrap_or_else(LocalTrackTagsError::task_failed),
            });
            continue; // A failed read supplies no evidence of tag absence.
        };
        physical_tracks_read += 1;
        let artist_code = match tags.album_artist.as_deref() {
            None => Some(FindingCode::AlbumArtistAbsent),
            Some(value) if value.trim().is_empty() => Some(FindingCode::AlbumArtistEmpty),
            _ => None,
        };
        if let Some(code) = artist_code {
            findings.push(finding(
                code,
                FindingLevel::Advisory,
                vec![evidence(uri, Field::AlbumArtist, tags.album_artist.clone())],
            ));
        }
        if let Some(date) = &tags.date {
            let value = evidence(uri, Field::Date, Some(date.clone()));
            if let Some(year) = interpreted_year(date) {
                years.entry(year).or_default().push(value);
            } else {
                findings.push(finding(
                    FindingCode::DateUninterpretable,
                    FindingLevel::Advisory,
                    vec![value],
                ));
            }
        }
        for (field, number) in [
            (Field::TrackNumber, tags.track_number),
            (Field::DiscNumber, tags.disc_number),
        ] {
            if number.is_none_or(|value| value == 0) {
                findings.push(finding(
                    FindingCode::NumberIndeterminate,
                    FindingLevel::Advisory,
                    vec![evidence(uri, field, number.map(|value| value.to_string()))],
                ));
            }
        }
        for (code, number_field, total_field, number, total) in [
            (
                FindingCode::TrackExceedsTotal,
                Field::TrackNumber,
                Field::TrackTotal,
                tags.track_number,
                tags.track_total,
            ),
            (
                FindingCode::DiscExceedsTotal,
                Field::DiscNumber,
                Field::DiscTotal,
                tags.disc_number,
                tags.disc_total,
            ),
        ] {
            if let (Some(number), Some(total)) = (number, total) {
                if total > 0 && number > total {
                    findings.push(finding(
                        code,
                        FindingLevel::Inconsistency,
                        vec![
                            evidence(uri, number_field, Some(number.to_string())),
                            evidence(uri, total_field, Some(total.to_string())),
                        ],
                    ));
                }
            }
        }
        if let (Some(disc), Some(number)) = (tags.disc_number, tags.track_number) {
            if disc > 0 && number > 0 {
                numbers.entry((disc, number)).or_default().extend([
                    evidence(uri, Field::DiscNumber, Some(disc.to_string())),
                    evidence(uri, Field::TrackNumber, Some(number.to_string())),
                ]);
            }
        }
    }
    if years.len() > 1 {
        findings.push(finding(
            FindingCode::YearDivergence,
            FindingLevel::Advisory,
            years.into_values().flatten().collect(),
        ));
    }
    for evidence in numbers.into_values().filter(|values| values.len() > 2) {
        findings.push(finding(
            FindingCode::PossibleNumberConflict,
            FindingLevel::PossibleConflict,
            evidence,
        ));
    }
    let physical_track_count = tracks.len() - cue_virtual_count;
    let mut checks_not_performed = vec![
        UnperformedCheck::MissingTracks,
        UnperformedCheck::ContentDuplicates,
        UnperformedCheck::CanonicalizationDiscardedEntries,
    ];
    if cue_virtual_count > 0 {
        checks_not_performed.extend([
            UnperformedCheck::CueMetadata,
            UnperformedCheck::CueNumbering,
        ]);
    }
    let coverage = Coverage {
        canonical_track_count: tracks.len(),
        physical_track_count,
        physical_tracks_read,
        failed_physical_track_count: failures.len(),
        cue_virtual_count,
        physical_inspection_complete: failures.is_empty(),
        partial: !failures.is_empty() || cue_virtual_count > 0,
        checks_not_performed,
    };
    Report {
        album_id: album_id.to_string(),
        tracks,
        coverage,
        findings,
        failures,
    }
}

fn interpreted_year(original: &str) -> Option<u32> {
    let date = original.trim();
    if !matches!(date.len(), 4 | 7 | 10) {
        return None;
    }
    if !date.bytes().enumerate().all(|(index, byte)| {
        if index == 4 || index == 7 {
            byte == b'-'
        } else {
            byte.is_ascii_digit()
        }
    }) {
        return None;
    }
    let year: u32 = date[..4].parse().ok()?;
    if year == 0 {
        return None;
    }
    if date.len() >= 7 {
        let month: u32 = date[5..7].parse().ok()?;
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        if date.len() == 10 {
            let day: u32 = date[8..10].parse().ok()?;
            if day == 0 || day > days {
                return None;
            }
        }
    }
    Some(year)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{LocalAlbumDisc, LocalItem};

    fn readable_tags() -> LocalTrackTags {
        LocalTrackTags {
            title: None,
            artist: None,
            album: None,
            genre: None,
            album_artist: Some("Artist".into()),
            date: Some("2026".into()),
            track_number: Some(1),
            track_total: Some(10),
            disc_number: Some(1),
            disc_total: Some(2),
            format: "flac".into(),
            readable: true,
            writable: true,
            editable: true,
            unsupported_reason: None,
            revision: "synthetic".into(),
        }
    }

    fn track(uri: &str, tags: LocalTrackTags) -> LocalAlbumTrackInspection {
        LocalAlbumTrackInspection {
            uri: uri.into(),
            kind: LocalAlbumTrackKind::Physical,
            tags: Some(tags),
            issue: None,
        }
    }

    fn with_code(report: &Report, code: FindingCode) -> Vec<&Finding> {
        report
            .findings
            .iter()
            .filter(|finding| finding.code == code)
            .collect()
    }

    fn group(disc: u32, uris: &[&str]) -> LocalAlbumDiscTracks {
        LocalAlbumDiscTracks {
            disc: LocalAlbumDisc {
                number: disc,
                label: format!("CD{disc}"),
                folder_path: "Music/Album".into(),
                track_count: uris.len(),
            },
            tracks: uris
                .iter()
                .map(|uri| LocalItem {
                    item_type: "file".into(),
                    path: (*uri).into(),
                    name: (*uri).into(),
                    title: None,
                    artist: None,
                    album: None,
                    duration: None,
                })
                .collect(),
        }
    }

    #[test]
    fn absent_and_empty_album_artist_are_advisory_with_original_evidence() {
        for (artist, code) in [
            (None, FindingCode::AlbumArtistAbsent),
            (Some(""), FindingCode::AlbumArtistEmpty),
            (Some(" \t"), FindingCode::AlbumArtistEmpty),
        ] {
            let mut tags = readable_tags();
            tags.album_artist = artist.map(str::to_string);
            let report = diagnose("album", vec![track("Music/a.flac", tags)]);
            let findings = with_code(&report, code);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].level, FindingLevel::Advisory);
            assert_eq!(
                findings[0].evidence,
                vec![evidence(
                    "Music/a.flac",
                    Field::AlbumArtist,
                    artist.map(str::to_string)
                )]
            );
            assert!(!report.coverage.partial);
        }
    }

    #[test]
    fn equal_years_with_different_date_precision_do_not_diverge() {
        let tracks = ["2026", "2026-10", "2026-10-11"]
            .into_iter()
            .enumerate()
            .map(|(index, date)| {
                let mut tags = readable_tags();
                tags.date = Some(date.into());
                tags.track_number = Some(index as u32 + 1);
                track(&format!("Music/{index}.flac"), tags)
            })
            .collect();
        let report = diagnose("album", tracks);
        assert!(report.findings.is_empty());
        assert_eq!(
            report.tracks[2].tags.as_ref().unwrap().date.as_deref(),
            Some("2026-10-11")
        );
    }

    #[test]
    fn year_divergence_preserves_dates_and_excludes_uninterpretable_values() {
        let tracks = [Some("1999"), Some(" 2000-02-29 "), Some("not a date"), None]
            .into_iter()
            .enumerate()
            .map(|(index, date)| {
                let mut tags = readable_tags();
                tags.date = date.map(str::to_string);
                tags.track_number = Some(index as u32 + 1);
                track(&format!("Music/{index}.flac"), tags)
            })
            .collect();
        let report = diagnose("album", tracks);
        let divergence = with_code(&report, FindingCode::YearDivergence);
        assert_eq!(divergence.len(), 1);
        assert_eq!(divergence[0].level, FindingLevel::Advisory);
        assert_eq!(
            divergence[0].evidence,
            vec![
                evidence("Music/0.flac", Field::Date, Some("1999".into())),
                evidence("Music/1.flac", Field::Date, Some(" 2000-02-29 ".into())),
            ]
        );
        assert_eq!(
            with_code(&report, FindingCode::DateUninterpretable)[0].evidence[0].value,
            Some("not a date".into())
        );
        assert_eq!(
            report.tracks[2].tags.as_ref().unwrap().date.as_deref(),
            Some("not a date")
        );
        assert_eq!(report.tracks[3].tags.as_ref().unwrap().date, None);
    }

    #[test]
    fn date_parser_rejects_invalid_and_unsupported_dates_without_inventing_years() {
        for value in [
            "",
            " ",
            "0000",
            "2026-00",
            "2026-13",
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-01-00",
            "2026suffix",
            "26",
            "二〇二六",
            "2026-10-11T12:00:00Z",
        ] {
            assert_eq!(interpreted_year(value), None, "{value}");
        }
        assert_eq!(interpreted_year("2000-02-29"), Some(2000));
        let mut tags = readable_tags();
        tags.date = Some("".into());
        let report = diagnose("album", vec![track("empty", tags)]);
        assert_eq!(
            with_code(&report, FindingCode::DateUninterpretable)[0].evidence[0].value,
            Some("".into())
        );
    }

    #[test]
    fn repetition_between_discs_is_legitimate_but_same_positive_pair_is_possible_conflict() {
        let mut disc_two = readable_tags();
        disc_two.disc_number = Some(2);
        let report = diagnose(
            "album",
            vec![
                track("CD1/a", readable_tags()),
                track("CD2/a", disc_two.clone()),
            ],
        );
        assert!(report.findings.is_empty());
        let report = diagnose(
            "album",
            vec![
                track("CD1/a", readable_tags()),
                track("CD2/a", disc_two.clone()),
                track("CD2/b", disc_two),
            ],
        );
        let conflicts = with_code(&report, FindingCode::PossibleNumberConflict);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].level, FindingLevel::PossibleConflict);
        assert_eq!(
            conflicts[0].evidence,
            vec![
                evidence("CD2/a", Field::DiscNumber, Some("2".into())),
                evidence("CD2/a", Field::TrackNumber, Some("1".into())),
                evidence("CD2/b", Field::DiscNumber, Some("2".into())),
                evidence("CD2/b", Field::TrackNumber, Some("1".into())),
            ]
        );
    }

    #[test]
    fn number_greater_than_positive_total_is_inconsistent_but_zero_total_is_not() {
        let mut tags = readable_tags();
        tags.track_number = Some(11);
        tags.disc_number = Some(3);
        let report = diagnose("album", vec![track("Music/a", tags.clone())]);
        for (code, number_field, total_field, number, total) in [
            (
                FindingCode::TrackExceedsTotal,
                Field::TrackNumber,
                Field::TrackTotal,
                "11",
                "10",
            ),
            (
                FindingCode::DiscExceedsTotal,
                Field::DiscNumber,
                Field::DiscTotal,
                "3",
                "2",
            ),
        ] {
            let findings = with_code(&report, code);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].level, FindingLevel::Inconsistency);
            assert_eq!(
                findings[0].evidence,
                vec![
                    evidence("Music/a", number_field, Some(number.into())),
                    evidence("Music/a", total_field, Some(total.into()))
                ]
            );
        }
        for total in [None, Some(0)] {
            tags.track_total = total;
            tags.disc_total = total;
            assert!(diagnose("album", vec![track("a", tags.clone())])
                .findings
                .is_empty());
        }
    }

    #[test]
    fn unobtained_or_zero_numbers_are_indeterminate_and_never_assume_disc_one() {
        for number in [None, Some(0)] {
            let mut tags = readable_tags();
            tags.disc_number = number;
            tags.track_number = number;
            let report = inspect_album_with(
                "album",
                || Ok(Some(vec![group(1, &["a", "b"])])),
                |_| Ok(tags.clone()),
            )
            .unwrap();
            assert_eq!(
                with_code(&report, FindingCode::NumberIndeterminate).len(),
                4
            );
            assert!(with_code(&report, FindingCode::PossibleNumberConflict).is_empty());
            assert!(report
                .findings
                .iter()
                .all(|finding| finding.level == FindingLevel::Advisory));
            assert_eq!(report.tracks[0].tags.as_ref().unwrap().disc_number, number);
        }
        let mut tags = readable_tags();
        tags.disc_number = None; // Repeated positive track alone is insufficient.
        let report = diagnose("album", vec![track("a", tags.clone()), track("b", tags)]);
        assert!(with_code(&report, FindingCode::PossibleNumberConflict).is_empty());
    }

    #[test]
    fn partial_reads_keep_operational_failures_separate_from_findings() {
        let mut tags = readable_tags();
        tags.album_artist = None;
        let report = inspect_album_with(
            "album",
            || Ok(Some(vec![group(1, &["read", "failed"])])),
            |uri| {
                if uri == "read" {
                    Ok(tags.clone())
                } else {
                    Err(LocalTrackTagsError::task_failed())
                }
            },
        )
        .unwrap();
        assert_eq!(report.coverage.canonical_track_count, 2);
        assert_eq!(report.coverage.physical_tracks_read, 1);
        assert_eq!(report.coverage.failed_physical_track_count, 1);
        assert!(!report.coverage.physical_inspection_complete);
        assert!(report.coverage.partial);
        assert_eq!(report.failures[0].uri, "failed");
        assert_eq!(with_code(&report, FindingCode::AlbumArtistAbsent).len(), 1);
        assert!(report
            .findings
            .iter()
            .flat_map(|finding| &finding.evidence)
            .all(|evidence| evidence.uri == "read"));
    }

    #[test]
    fn no_readable_tracks_is_not_a_clean_diagnosis() {
        let report = inspect_album_with(
            "album",
            || Ok(Some(vec![group(1, &["failed"])])),
            |_| Err(LocalTrackTagsError::task_failed()),
        )
        .unwrap();
        assert!(report.findings.is_empty());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.coverage.physical_tracks_read, 0);
        assert!(report.coverage.partial);
        assert!(!report.coverage.physical_inspection_complete);
    }

    #[test]
    fn cue_presence_is_partial_even_when_all_physical_reads_succeed() {
        for uris in [
            vec!["Music/Album/image.CUE/track0001"],
            vec!["Music/Album/a.flac", "Music/Album/image.CUE/track0001"],
        ] {
            let report = inspect_album_with(
                "album",
                || Ok(Some(vec![group(1, &uris)])),
                |uri| {
                    assert!(
                        !uri.contains(".CUE"),
                        "CUE must not be physically inspected"
                    );
                    Ok(readable_tags())
                },
            )
            .unwrap();
            assert!(report.coverage.physical_inspection_complete);
            assert!(report.coverage.partial);
            assert_eq!(report.coverage.cue_virtual_count, 1);
            assert_eq!(report.coverage.physical_tracks_read, uris.len() - 1);
            assert!(report
                .coverage
                .checks_not_performed
                .contains(&UnperformedCheck::CueMetadata));
            assert!(report
                .coverage
                .checks_not_performed
                .contains(&UnperformedCheck::CueNumbering));
            assert!(report.failures.is_empty());
            assert_eq!(
                with_code(&report, FindingCode::CueChecksNotPerformed)[0].evidence[0],
                evidence(
                    "Music/Album/image.CUE/track0001",
                    Field::Cue,
                    Some("present".into())
                )
            );
        }
    }

    #[test]
    fn catalog_failure_and_missing_membership_stop_before_physical_reads() {
        for (resolution, error) in [
            (
                Err("update or catalog failure".into()),
                DoctorError::CatalogReadFailed,
            ),
            (Ok(None), DoctorError::AlbumNotFound),
            (Ok(Some(vec![group(1, &[])])), DoctorError::AlbumNotFound),
        ] {
            assert_eq!(
                inspect_album_with("album", || resolution, |_| panic!("must not inspect")),
                Err(error)
            );
        }
    }

    #[test]
    fn membership_uses_every_canonical_group_without_catalog_disc_inference() {
        let mut visited = Vec::new();
        let report = inspect_album_with(
            "album",
            || Ok(Some(vec![group(1, &["a"]), group(2, &["b"])])),
            |uri| {
                visited.push(uri.to_string());
                let mut tags = readable_tags();
                tags.disc_number = None;
                Ok(tags)
            },
        )
        .unwrap();
        assert_eq!(visited, ["a", "b"]);
        assert_eq!(report.coverage.canonical_track_count, 2);
        assert_eq!(
            with_code(&report, FindingCode::NumberIndeterminate).len(),
            2
        );
        assert!(with_code(&report, FindingCode::PossibleNumberConflict).is_empty());
        assert!(report
            .coverage
            .checks_not_performed
            .contains(&UnperformedCheck::CanonicalizationDiscardedEntries));
    }

    #[test]
    fn serialized_codes_are_stable_and_errors_are_not_success_reports() {
        for (code, name) in [
            (FindingCode::AlbumArtistAbsent, "album_artist_absent"),
            (FindingCode::AlbumArtistEmpty, "album_artist_empty"),
            (FindingCode::YearDivergence, "year_divergence"),
            (FindingCode::DateUninterpretable, "date_uninterpretable"),
            (FindingCode::NumberIndeterminate, "number_indeterminate"),
            (FindingCode::TrackExceedsTotal, "track_exceeds_total"),
            (FindingCode::DiscExceedsTotal, "disc_exceeds_total"),
            (
                FindingCode::PossibleNumberConflict,
                "possible_number_conflict",
            ),
            (
                FindingCode::CueChecksNotPerformed,
                "cue_checks_not_performed",
            ),
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), name);
        }
        for (error, name) in [
            (DoctorError::StateUnavailable, "state_unavailable"),
            (DoctorError::CatalogReadFailed, "catalog_read_failed"),
            (DoctorError::AlbumNotFound, "album_not_found"),
            (DoctorError::TaskFailed, "task_failed"),
        ] {
            assert_eq!(serde_json::to_value(error).unwrap(), name);
        }
    }

    #[test]
    fn actual_readable_nonwritable_wav_is_diagnosed_without_modifying_the_fixture() {
        use std::fs;
        use std::os::unix::fs::symlink;
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(
            std::env::temp_dir().join(format!("sonante-doctor-wav-{}", std::process::id())),
        );
        fs::create_dir(&fixture.0).unwrap();
        let source = fixture.0.join("Music");
        let library = fixture.0.join("library");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&library).unwrap();
        symlink(&source, library.join("Music")).unwrap();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(40_u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes()); // PCM
        bytes.extend(1_u16.to_le_bytes()); // mono
        bytes.extend(44100_u32.to_le_bytes());
        bytes.extend(88200_u32.to_le_bytes());
        bytes.extend(2_u16.to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(4_u32.to_le_bytes());
        bytes.extend([0; 4]);
        let file = source.join("a.wav");
        fs::write(&file, &bytes).unwrap();
        let report = inspect_album_with(
            "album",
            || Ok(Some(vec![group(1, &["Music/a.wav"])])),
            |uri| {
                tags::inspect_local_track_tags(
                    &library,
                    uri,
                    &[source.to_string_lossy().into_owned()],
                )
            },
        )
        .unwrap();
        let tags = report.tracks[0].tags.as_ref().unwrap();
        assert!(tags.readable);
        assert!(!tags.writable);
        assert_eq!(tags.format, "wav");
        assert!(report.failures.is_empty());
        assert!(report.coverage.physical_inspection_complete);
        assert_eq!(with_code(&report, FindingCode::AlbumArtistAbsent).len(), 1);
        assert_eq!(fs::read(file).unwrap(), bytes);
        assert_eq!(fs::read_dir(&source).unwrap().count(), 1);
    }
}
