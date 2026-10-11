/// Merge metadata without selecting a dominant genre or splitting arbitrary tag text.
pub(crate) fn add_genre(genres: &mut Vec<String>, value: &str) {
    let display = value.trim();
    if display.is_empty()
        || display.len() > 4096
        || value.chars().any(char::is_control)
        || display.contains("://")
        || display.to_lowercase().contains("x-plex-token")
    {
        return;
    }
    let key = display
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if !genres.iter().any(|genre| {
        genre
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            == key
    }) {
        genres.push(display.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_genres_keep_original_names_and_never_split_or_invent_metadata() {
        let mut genres = Vec::new();
        for value in [
            "Jazz",
            "jazz",
            "Música  Popular",
            "música popular",
            "Pop / Rock",
            "",
            "bad\nvalue",
            "https://host/?X-Plex-Token=fixture",
        ] {
            add_genre(&mut genres, value);
        }
        assert_eq!(genres, ["Jazz", "Música  Popular", "Pop / Rock"]);
    }
}
