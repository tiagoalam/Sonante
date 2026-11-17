//! Original Linux0.6.2 log-only redaction policy, byte-exact. No DEV collector.
use std::fmt::Display;
pub(crate) fn error_summary(error: impl Display) -> String {
    let text = error.to_string();
    let lower = text.to_lowercase();
    // Hints describe text matches, not proven causes. Only fixed labels and parsed
    // numeric codes reach the terminal. Raw errors may
    // contain authenticated URLs, filenames, metadata or application UI messages.
    let categories = [
        ("timeout", &["timeout", "timed out", "tempo limite"][..]),
        (
            "permission_denied",
            &["permission denied", "permissão", "permissao"][..],
        ),
        (
            "not_found",
            &[
                "not found",
                "no such file",
                "não encontrado",
                "não encontrada",
            ][..],
        ),
        (
            "connection_refused",
            &["connection refused", "conexão recusada"][..],
        ),
        ("connection_reset", &["connection reset", "broken pipe"][..]),
        (
            "unexpected_eof",
            &["eof", "unexpected end", "resposta incompleta"][..],
        ),
        ("busy", &["busy", "ocupado", "em uso"][..]),
        (
            "invalid_state",
            &["invalid", "inválid", "incompat", "inconsisten"][..],
        ),
        (
            "conflict",
            &["conflict", "conflito", "identidade", "identity"][..],
        ),
        ("lock_poisoned", &["poison", "corrompido"][..]),
        (
            "authentication",
            &["unauthorized", "forbidden", "autenticação"][..],
        ),
        ("task_panicked", &["panic"][..]),
        ("not_seekable", &["not seekable"][..]),
        (
            "uncertain",
            &["uncertain", "unconfirmed", "incerto", "não confirmado"][..],
        ),
        ("unsupported", &["unsupported", "não suportad"][..]),
        ("unavailable", &["unavailable", "indisponível"][..]),
    ];
    // Exact, fixed application codes add useful context without copying the
    // surrounding error or changing its IPC/guard semantics.
    let candidate = text.split(|c: char| c.is_whitespace() || c == ':' || c == ';').next().unwrap_or("");
    let code_hint = match candidate {
        "SONANTE_RUNTIME_MISSING" => Some("not_found"),
        "MPD_STARTUP_PROBE_TIMEOUT" => Some("timeout"),
        "MPD_STARTUP_PROBE_OUTPUT_LIMIT" => Some("output_limit"),
        "MPD_STARTUP_PROBE_UNCONFIRMED" | "MPD_STARTUP_PROBE_CLEANUP_UNCONFIRMED" => Some("uncertain"),
        "SONANTE_RUNTIME_UNTRUSTED" => Some("untrusted_runtime"),
        "SONANTE_RUNTIME_OVERRIDE_FORBIDDEN" => Some("runtime_override_forbidden"),
        "FIRST_RUN_EXISTING_LIBRARY_REQUIRES_REVIEW" => Some("existing_library"),
        "FIRST_RUN_CONFIG_CHANGED" => Some("conflict"),
        "FIRST_RUN_MANAGED_SAVE_UNCONFIRMED" | "FIRST_RUN_PREPARATION_INCOMPLETE" | "FIRST_RUN_DATABASE_UNCONFIRMED" => Some("uncertain"),
        "LIBRARY_RECOVERY_PROFILE_BUSY" | "CONFIG_SAVE_BUSY" => Some("busy"),
        "LIBRARY_RECOVERY_UNSAFE_FILESYSTEM" => Some("unsafe_filesystem"),
        "LIBRARY_RECOVERY_PLAN_CHANGED" | "NAVIDROME_CONTEXT_CHANGED" => Some("conflict"),
        "LIBRARY_RECOVERY_CANCELLED" | "LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN" => Some("cancelled"),
        "LIBRARY_RECOVERY_SET_INCOMPLETE" => Some("incomplete"),
        "CONFIG_PERSISTENCE_UNCERTAIN" | "MANAGED_FOLDERS_SAVE_PENDING" => Some("uncertain"),
        "FIRST_RUN_CATALOG_INDEXING" | "LOCAL_ADDITION_INDEXING" => Some("indexing"),
        "NAVIDROME_AUTH" => Some("authentication"),
        "NAVIDROME_INVALID" | "SMART_PLAYLIST_INVALID" => Some("invalid_state"),
        "NAVIDROME_PROTOCOL" => Some("protocol"),
        "NAVIDROME_STORAGE" | "SMART_PLAYLIST_STORAGE" => Some("storage"),
        "NAVIDROME_RATE_LIMITED" => Some("rate_limited"),
        "NAVIDROME_UNAVAILABLE" | "SMART_PLAYLIST_READ" | "SMART_PLAYLIST_SOURCE" => Some("unavailable"),
        _ => None,
    };
    let kind = code_hint.or_else(|| categories
        .iter()
        .find(|(_, terms)| terms.iter().any(|term| lower.contains(term)))
        .map(|(kind, _)| *kind))
        .unwrap_or("unclassified");
    let mut summary = format!("error_hint={kind}");
    if code_hint.is_some() { summary.push_str(&format!(" code={candidate}")); }
    if let Some(code) = numeric_code(&text, "(os error ", ")") {
        summary.push_str(&format!(" os_code={code}"));
    }
    if let Some(code) = numeric_code(&text, "ACK [", "@") {
        summary.push_str(&format!(" mpd_ack_code={code}"));
    }
    summary
}

fn numeric_code(text: &str, prefix: &str, suffix: &str) -> Option<u32> {
    let value = text.split_once(prefix)?.1.split_once(suffix)?.0;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

