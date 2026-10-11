//! Log-only error summaries. IPC errors retain their original contracts and messages.
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
    let kind = categories
        .iter()
        .find(|(_, terms)| terms.iter().any(|term| lower.contains(term)))
        .map(|(kind, _)| *kind)
        .unwrap_or("unclassified");
    let mut summary = format!("error_hint={kind}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_keep_safe_context_without_private_data() {
        let error = "Falha: ACK [5@0] {add} https://user:password@server/private/song?X-Plex-Token=secret /home/private/library (os error 13)";
        assert_eq!(
            error_summary(error),
            "error_hint=unclassified os_code=13 mpd_ack_code=5"
        );
        assert_eq!(
            error_summary("Permission denied (os error 13)"),
            "error_hint=permission_denied os_code=13"
        );
        assert_eq!(
            error_summary("Falha: timeout /mnt/private"),
            "error_hint=timeout"
        );
    }

    #[test]
    fn raw_messages_and_malformed_codes_are_never_echoed() {
        for error in [
            "unknown secret",
            "ACK [secret@0]",
            "(os error token)",
            "(os error 99999999999999999999999)",
            "Falha na operação",
        ] {
            assert_eq!(error_summary(error), "error_hint=unclassified");
        }
    }

    #[test]
    fn portuguese_application_errors_use_english_log_hints_only() {
        assert_eq!(
            error_summary("Estado indisponível /private"),
            "error_hint=unavailable"
        );
        assert_eq!(
            error_summary("Falha de autenticação token=secret"),
            "error_hint=authentication"
        );
        assert_eq!(
            error_summary("Cache da fila inválido"),
            "error_hint=invalid_state"
        );
    }

    #[test]
    fn internal_log_templates_are_english_and_prefixed() {
        let sources = [
            include_str!("lib.rs"),
            include_str!("audio.rs"),
            include_str!("supervisor.rs"),
            include_str!("mpris.rs"),
            include_str!("analyzer.rs"),
            include_str!("alsa_mixer.rs"),
            include_str!("local_watcher.rs"),
            include_str!("artwork_progress.rs"),
            include_str!("local_media.rs"),
        ];
        for source in sources {
            // All existing terminal sites use println!/eprintln! with a literal template.
            for call in source.split("println!(").skip(1) {
                let template = call.split_once('"').unwrap().1.split_once('"').unwrap().0;
                assert!(template.is_ascii(), "Non-English log template: {template}");
                assert!(
                    template.starts_with('[') || template == "{}",
                    "Missing log prefix: {template}"
                );
                for portuguese in [
                    "Falha",
                    "indispon",
                    "Iniciando",
                    "Progresso",
                    "Usando",
                    "encerr",
                    "dispositivo=",
                ] {
                    assert!(
                        !template.contains(portuguese),
                        "Portuguese log template: {template}"
                    );
                }
            }
        }
    }
}
