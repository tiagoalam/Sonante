#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
MPD_EQ_SHA256="f7157589c34c8c98e8989112e1e53284dee6abdba83c5bf4b894592d134674b0"
CAMILLA_SHA256="802366b3eb8080506cba630e5cb70ed1a377d5100896dea8f7e2a4e9ff7c47ef"
PRIVATE_EQ_RUNTIME="${HOME:?}/.local/share/sonante-0.6.4-sqlite/runtime/eq-dev"

usage() {
  cat <<'EOF'
Uso:
  scripts/dev-eq.sh
  scripts/dev-eq.sh --verify-only
  scripts/dev-eq.sh /caminho/privado/mpd-eq /caminho/privado/camilladsp
  scripts/dev-eq.sh --verify-only /caminho/privado/mpd-eq /caminho/privado/camilladsp

Sem caminhos, usa os runtimes privados instalados em:
  ~/.local/share/sonante-0.6.4-sqlite/runtime/eq-dev/
Os arquivos precisam corresponder aos hashes fixados no código. --verify-only
valida identidade, proprietário e permissões sem iniciar o aplicativo.
EOF
}

if [[ "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

verify_only=0
if [[ "${1:-}" == "--verify-only" ]]; then
  verify_only=1
  shift
fi

if [[ "${SONANTE_064_TEST_NULL_OUTPUT:-}" == "1" ]]; then
  echo "EQ bloqueado: SONANTE_064_TEST_NULL_OUTPUT=1 força o modo null." >&2
  exit 2
fi

if [[ $# -eq 0 ]]; then
  set -- "$PRIVATE_EQ_RUNTIME/mpd-eq" "$PRIVATE_EQ_RUNTIME/camilladsp"
elif [[ $# -ne 2 ]]; then
  usage >&2
  exit 2
fi

verify_runtime() {
  local label="$1" path="$2" expected="$3" resolved actual owner

  if [[ "$path" != /* || ! -f "$path" || -L "$path" || ! -x "$path" ]]; then
    echo "$label inválido: informe um arquivo executável absoluto, regular e sem symlink." >&2
    return 2
  fi

  resolved="$(realpath -e -- "$path")" || return 2
  case "$resolved" in
    /usr/lib/sonante/runtimes|/usr/lib/sonante/runtimes/*)
      echo "$label recusado: o caminho pertence ao runtime instalado do Sonante." >&2
      return 2
      ;;
  esac

  owner="$(stat -c '%u' -- "$resolved")"
  if [[ "$owner" != "$(id -u)" ]]; then
    echo "$label recusado: o arquivo não pertence ao usuário atual." >&2
    return 2
  fi

  actual="$(sha256sum -- "$resolved" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "$label recusado: SHA-256 não corresponde ao runtime privado qualificado." >&2
    return 2
  fi
  printf '%s\n' "$resolved"
}

mpd_eq="$(verify_runtime 'MPD EQ' "$1" "$MPD_EQ_SHA256")" || exit $?
camilla="$(verify_runtime 'CamillaDSP' "$2" "$CAMILLA_SHA256")" || exit $?

if [[ "$verify_only" == "1" ]]; then
  printf 'MPD EQ privado validado: %s\n' "$mpd_eq"
  printf 'CamillaDSP privado validado: %s\n' "$camilla"
  exit 0
fi

cd -- "$ROOT"
export SONANTE_EQ_MPD_PATH="$mpd_eq"
export SONANTE_EQ_CAMILLA_PATH="$camilla"
export SONANTE_EQ_DIAGNOSTICS=1
printf 'EQ diagnostics: %s/config/sonante/eq-activation-diagnostics.jsonl\n' "${XDG_CONFIG_HOME:?private profile config directory must be set}"
exec npm run tauri -- dev
