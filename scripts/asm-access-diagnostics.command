#!/bin/bash
# Read-only macOS access diagnostics. Keep the failing app installed and running.
# No session bodies, environment dumps, permission changes, or uploads.
set -u
umask 077

if [[ "${1:-}" == "--help" ]]; then
  printf '%s\n' 'Usage: bash asm-access-diagnostics.command [--collect-only] [failing-file-path]' \
    'Default: capture a fresh GUI retry. --collect-only: collect existing logs without prompting.'
  exit 0
fi
if [[ "$(/usr/bin/uname -s)" != Darwin ]]; then
  printf '%s\n' 'This diagnostic requires macOS.' >&2
  exit 1
fi
asm_collect_only=false
if [[ "${1:-}" == "--collect-only" ]]; then
  asm_collect_only=true
  shift
fi
if (( $# > 1 )); then
  printf '%s\n' 'Expected at most one failing file path; quote paths containing spaces.' >&2
  exit 1
fi
asm_sample=${1:-}
asm_app='/Applications/Agent Session Manager.app'
asm_binary="$asm_app/Contents/MacOS/agent-session-manager"
asm_root="$HOME/.qoder/projects"
asm_output=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/asm-access-diagnostics.XXXXXXXX") || exit 1
asm_report="$asm_output/diagnostics.txt"
asm_stream_pid=''

stop_stream() {
  if [[ -n "$asm_stream_pid" ]]; then
    kill "$asm_stream_pid" 2>/dev/null || true
    wait "$asm_stream_pid" 2>/dev/null || true
    asm_stream_pid=''
  fi
}
trap stop_stream EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run() {
  local asm_label=$1 asm_status
  shift
  printf '\n--- %s ---\n' "$asm_label" >> "$asm_report"
  "$@" >> "$asm_report" 2>&1
  asm_status=$?
  printf 'command_exit: %s\n' "$asm_status" >> "$asm_report"
}

printf '%s\n' 'ASM access diagnostic v2' \
  'Scope: app identity, process ancestry, security extensions, path metadata, matching system logs.' \
  'Session contents and configuration values are not collected.' \
  'System extensions do not list every security product (some run as daemons).' \
  'A missing log entry does not rule out TCC, sandbox, or Endpoint Security enforcement.' > "$asm_report"
run 'Captured at (UTC)' /bin/date -u '+%Y-%m-%dT%H:%M:%SZ'
run 'macOS version' /usr/bin/sw_vers
run 'Architecture' /usr/bin/uname -m
run 'Effective user ID' /usr/bin/id -u
run 'App version' /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$asm_app/Contents/Info.plist"
run 'Executable SHA-256' /usr/bin/shasum -a 256 "$asm_binary"
run 'Signature identity and designated requirement' /usr/bin/codesign -d --verbose=4 -r- "$asm_app"
run 'Entitlements' /usr/bin/codesign -d --entitlements :- "$asm_app"
run 'Signature verification (all architectures)' /usr/bin/codesign --verify --strict --all-architectures --verbose=2 "$asm_app"
run 'System extensions (installed does not prove an extension denied access)' /usr/bin/systemextensionsctl list
run 'Device management status' /usr/bin/profiles status -type enrollment

printf '\n--- App processes and their ancestors (no command arguments) ---\n' >> "$asm_report"
asm_pids=$(/bin/ps -axo pid=,comm= | /usr/bin/awk -v exe="$asm_binary" '{ pid=$1; $1=""; sub(/^[[:space:]]+/, ""); if ($0==exe) print pid }')
if [[ -z "$asm_pids" ]]; then
  printf '%s\n' 'No installed app process found. Keep the failing GUI running for a fresh capture.' >> "$asm_report"
fi
for asm_pid in $asm_pids; do
  for asm_depth in 1 2 3 4; do
    /bin/ps -ww -p "$asm_pid" -o pid=,ppid=,uid=,comm= >> "$asm_report" 2>&1
    asm_pid=$(/bin/ps -p "$asm_pid" -o ppid= | /usr/bin/tr -d ' ')
    [[ "$asm_pid" =~ ^[0-9]+$ && "$asm_pid" -gt 1 ]] || break
  done
done

# Metadata inspection never opens a session for reading, including a FIFO or symlink.
run 'Qoder directory attributes and ACLs' /bin/ls -ldeO@ "$HOME/.qoder" "$asm_root"
if [[ -z "$asm_sample" ]]; then
  asm_sample=$(/usr/bin/find -H "$asm_root" -type f -name '*.jsonl' -print -quit 2>/dev/null)
  printf '\nsample_selection: first regular JSONL; may differ from the GUI failure\n' >> "$asm_report"
else
  printf '\nsample_selection: caller-supplied failure path\n' >> "$asm_report"
fi
if [[ -n "$asm_sample" ]]; then
  run 'Sample path attributes (metadata only)' /bin/ls -ldeO@ "$(/usr/bin/dirname "$asm_sample")" "$asm_sample"
  run 'Sample target metadata (follow symlinks)' /usr/bin/stat -L -f 'path=%N type=%HT mode=%Sp uid=%u gid=%g flags=%Sf size=%z' "$asm_sample"
  printf '\n--- Resolved parent directory ---\n' >> "$asm_report"
  (cd -P "$(/usr/bin/dirname "$asm_sample")" && pwd -P) >> "$asm_report" 2>&1
fi

# Match the affected paths/identity across all emitters: OS-only filtering misses
# security products' own denial records. Debug events may exist only in the live stream.
asm_predicate='eventMessage CONTAINS[c] "agent-session" OR eventMessage CONTAINS[c] "Agent Session Manager" OR eventMessage CONTAINS[c] "/.qoder/" OR eventMessage CONTAINS[c] "asm-access-diagnostics"'
if [[ "$asm_collect_only" == false ]]; then
  printf '\n--- Live system logs during GUI retry ---\n' >> "$asm_report"
  /usr/bin/log stream --timeout 180 --style compact --level debug --predicate "$asm_predicate" >> "$asm_report" 2>&1 &
  asm_stream_pid=$!
  printf '\n%s\n' 'Keep the current app open. Click Retry scan / Retry read, then return here.' \
    'Press Enter after the error appears (capture ends after 180 seconds).'
  if ! read -r -t 180 asm_response; then
    printf '\nretry_confirmation: timed out or stdin unavailable\n' >> "$asm_report"
  else
    printf '\nretry_confirmation: user confirmed a GUI retry\n' >> "$asm_report"
  fi
  stop_stream
fi
run 'Matching persisted system logs (last 3 minutes)' /usr/bin/log show --last 3m --style compact --info --debug --predicate "$asm_predicate"
run 'Finished at (UTC)' /bin/date -u '+%Y-%m-%dT%H:%M:%SZ'
printf '\nReport: %s\n' "$asm_report"
printf '%s\n' 'Review paths, app names, and system log messages before sharing. No data was uploaded.'
if [[ "$asm_collect_only" == false ]]; then
  /usr/bin/open -R "$asm_report" 2>/dev/null || true
fi
