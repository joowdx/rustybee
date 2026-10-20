#!/usr/bin/env bash
# Android 11 smoke test (AC-011-13): pair a running API 30 emulator with a local
# server, send one SMS through the API and receive one. Catches what lint and
# the unit tests can't see on old releases, such as a system service that is
# null there or a permission only the old code path needs.
#
# Usage: api30-smoke.sh <server binary> <foss debug apk>
set -euo pipefail

server_bin=$1
apk=$2
port=8111
work=$(mktemp -d)
db=$work/data/rustybee.db
api_key=rbk_ci_smoke_key_local_only
pair_code=CISMOKE30

sha256() { if command -v sha256sum >/dev/null; then printf %s "$1" | sha256sum | cut -d' ' -f1; else printf %s "$1" | shasum -a 256 | cut -d' ' -f1; fi; }
now_ms() { echo $(( $(date +%s) * 1000 )); }
fail() { echo "::error::$*"; adb logcat -d -s rustybee:V AndroidRuntime:E | tail -40 || true; exit 1; }

# Polls a command until it succeeds: wait_for <seconds> <description> <command...>
wait_for() {
  local secs=$1 what=$2; shift 2
  for _ in $(seq "$secs"); do "$@" && return 0; sleep 1; done
  fail "timed out: $what"
}

screen() { adb exec-out uiautomator dump /dev/tty 2>/dev/null; }
shown() { screen | grep -qF "text=\"$1\""; }

# Taps the first on-screen element whose text is exactly $1; fails if none.
tap() {
  local b
  b=$(screen | grep -oE "text=\"$1\"[^>]*bounds=\"[^\"]*\"" | head -1 | grep -oE 'bounds="[^"]*"' | grep -oE '[0-9]+' | tr '\n' ' ')
  [ -n "$b" ] || return 1
  set -- $b
  adb shell input tap $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 ))
}

# --- server
mkdir -p "$work/data"
export RUSTYBEE_DATA_DIR=$work/data RUSTYBEE_BIND=0.0.0.0:$port RUSTYBEE_PUBLIC_URL=http://10.0.2.2:$port
printf 'ci-smoke-password\n' | "$server_bin" set-password --stdin >/dev/null
"$server_bin" serve >"$work/serve.log" 2>&1 &
server_pid=$!
trap 'kill $server_pid 2>/dev/null || true; rm -rf "$work"' EXIT
wait_for 30 "server health" curl -fs "http://127.0.0.1:$port/api/v1/health" -o /dev/null

# Test-only rows written straight to the database: an API key and a pairing code.
t=$(now_ms)
sqlite3 "$db" "INSERT INTO api_keys(public_id,name,prefix,token_sha256,created_at) VALUES('ci-smoke','ci','rbk_ci',X'$(sha256 "$api_key")',$t);
  INSERT INTO pairing_codes(code_sha256,created_at,expires_at) VALUES(X'$(sha256 "$pair_code")',$t,$((t + 600000)));"

# --- app
[ "$(adb shell getprop ro.build.version.sdk | tr -d '\r')" = 30 ] || fail "emulator is not API 30"
adb install -r -g "$apk" >/dev/null
adb logcat -c
adb shell am start -W -a android.intent.action.VIEW \
  -d "rustybee://pair?u=http%3A%2F%2F10.0.2.2%3A$port\&c=$pair_code" dev.joowdx.rustybee >/dev/null
wait_for 20 "setup screen" tap "Allow insecure http \(LAN\)"
wait_for 10 "pair button" tap "Pair"
wait_for 10 "pair confirmation" shown "Pair with this server?"
# The dialog may still be animating in; tap until it is gone.
confirmed() { shown "Pair with this server?" || return 0; tap "Pair" || true; sleep 1; ! shown "Pair with this server?"; }
wait_for 15 "pair confirmed" confirmed

device_id() { sqlite3 "$db" "SELECT public_id FROM devices WHERE connected=1 AND removed_at IS NULL LIMIT 1;"; }
wait_for 60 "device connected" test -n "$(device_id)"
dev=$(device_id)
echo "paired: $dev"

# --- send (the API 30 SmsManager path)
curl -fsS -X POST "http://127.0.0.1:$port/api/v1/messages" \
  -H "Authorization: Bearer $api_key" -H 'Content-Type: application/json' \
  -d "{\"to\":[\"+15551230030\"],\"body\":\"ci api30 send\",\"device_id\":\"$dev\"}" >/dev/null
out_state() { sqlite3 "$db" "SELECT state FROM messages WHERE direction='out' ORDER BY id DESC LIMIT 1;"; }
sent() { case "$(out_state)" in sent|delivered) return 0 ;; failed) fail "send failed: $(sqlite3 "$db" "SELECT error_message FROM messages WHERE direction='out' ORDER BY id DESC LIMIT 1;")" ;; *) return 1 ;; esac; }
wait_for 60 "message sent" sent
echo "sent: $(out_state)"

# --- receive
adb emu sms send 5559876 "ci api30 inbound" >/dev/null
received() { [ "$(sqlite3 "$db" "SELECT count(*) FROM messages WHERE direction='in' AND body='ci api30 inbound';")" -ge 1 ]; }
wait_for 60 "message received" received
echo "received"

# --- the hello has the exit reason and no start reason below API 35
lifecycle=$(sqlite3 "$db" "SELECT coalesce(lifecycle_json,'') FROM devices WHERE public_id='$dev';")
case "$lifecycle" in *'"start"'*) fail "lifecycle has a start reason on API 30: $lifecycle" ;; esac

if adb logcat -d | grep -q "FATAL EXCEPTION"; then fail "the app crashed"; fi
echo "API 30 smoke test passed"
