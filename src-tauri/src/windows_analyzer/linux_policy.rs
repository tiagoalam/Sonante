// Original Linux0.6.2 DSD/DoP policy, copied byte-exact except visibility.
// GPL-3.0-only; see the original Sonante LICENSE and AUTHORS.
pub(super) fn dsd_or_dop_reason(status: &[String], current_song: &[String]) -> Option<&'static str> {
    let format_is_dsd = status
        .iter()
        .any(|line| line.strip_prefix("audio: ").is_some_and(is_dsd_or_dop))
        || current_song
            .iter()
            .any(|line| line.strip_prefix("Format: ").is_some_and(is_dsd_or_dop));
    let file_is_dsd = current_song.iter().any(|line| {
        line.strip_prefix("file: ").is_some_and(|value| {
            let clean = value.split(['?', '#']).next().unwrap_or(value);
            matches!(
                clean
                    .rsplit('.')
                    .next()
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("dsf" | "dff" | "dsd")
            )
        })
    });
    if format_is_dsd {
        Some("current_audio_format_is_dsd")
    } else if file_is_dsd {
        Some("current_file_is_dsd")
    } else {
        None
    }
}

fn is_dsd_or_dop(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("dsd") || value.contains("dop")
}
