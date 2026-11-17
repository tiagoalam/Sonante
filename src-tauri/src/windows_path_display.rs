//! Presentation only: never use the displayed string as file admission authority.
use std::path::Path;
pub(crate) fn display(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(network) = value.strip_prefix("\\\\?\\UNC\\") {
        return format!("//{}", network.replace('\\', "/"));
    }
    value
        .strip_prefix("\\\\?\\")
        .unwrap_or(&value)
        .replace('\\', "/")
}
#[cfg(test)]
mod tests {
    use super::display;
    #[test]
    fn canonical_unc_authority_survives_preferences_and_picker_display() {
        for (raw, expected) in [
            (r"\\?\UNC\server\Music\Música", "//server/Music/Música"),
            (r"\\?\UNC\server\Music\", "//server/Music/"),
            (r"\\server\Music\Música", "//server/Music/Música"),
        ] {
            let shown = display(std::path::Path::new(raw));
            assert_eq!(shown, expected);
            assert_eq!(display(std::path::Path::new(&shown)), shown);
        }
    }
    #[test]
    fn drive_paths_keep_the_previous_display_and_no_network_authority_is_invented() {
        for (raw, expected) in [
            (r"\\?\C:\Music\Música", "C:/Music/Música"),
            (r"Z:\Music", "Z:/Music"),
            ("C:/Music", "C:/Music"),
            ("relative/music", "relative/music"),
        ] {
            assert_eq!(display(std::path::Path::new(raw)), expected);
        }
    }
    #[cfg(windows)]
    #[test]
    fn shown_unc_is_absolute_and_retains_server_share_for_native_path_parsing() {
        use std::path::{Component, Path, Prefix};
        let shown = display(std::path::Path::new(r"\\?\UNC\server\Music\Música"));
        let path = Path::new(&shown);
        assert!(path.is_absolute());
        assert!(
            matches!(path.components().next(),Some(Component::Prefix(p))if matches!(p.kind(),Prefix::UNC(server,share) if server=="server"&&share=="Music"))
        );
    }
}
