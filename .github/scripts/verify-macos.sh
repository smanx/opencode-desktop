#!/usr/bin/env bash
# Verify the installed OpenCode Desktop app on macOS:
#   launch the app, confirm it keeps running, confirm the opencode server +
#   web UI is reachable, capture a screenshot, and emit a PASS/FAIL verdict.
set -u

PORT=4096
TIMEOUT=240

APP_PATH="/Applications/opencode-desktop.app"
BIN="$APP_PATH/Contents/MacOS/opencode-desktop"

if [ ! -x "$BIN" ]; then
  echo "::error::Installed app binary not found at $BIN."
  exit 1
fi

# Launch via LaunchServices (`open`), the proper way to start a .app bundle.
# Launching the bundle's raw binary directly makes macOS kill that instance when
# `open` (used to bring the window to the front for screenshots) takes it over.
echo "Launching installed app: $APP_PATH"

# `open`-launched apps do not inherit the runner's PATH, so opencode (installed
# under the hosted tool cache) would be invisible to the app's find_opencode and
# to the opencode it spawns. Symlink them into /usr/local/bin, which the app
# searches.
if command -v opencode >/dev/null 2>&1; then
  sudo ln -sf "$(command -v opencode)" /usr/local/bin/opencode || true
fi
if command -v node >/dev/null 2>&1; then
  sudo ln -sf "$(command -v node)" /usr/local/bin/node || true
fi

open "$APP_PATH"

# find the app process (managed by LaunchServices; track it by name)
APP_PID=""
for i in $(seq 1 20); do
  APP_PID="$(pgrep -f "$APP_PATH/Contents/MacOS" | head -1 || true)"
  [ -n "$APP_PID" ] && break
  sleep 1
done
echo "App PID: ${APP_PID:-unknown}"

app_alive() {
  [ -n "$(pgrep -f "$APP_PATH/Contents/MacOS" || true)" ]
}

# (e) screenshot timeline: at launch (t=0), then at 5s and 10s
take_screenshot() {
  local out="$1"
  open "$APP_PATH" 2>/dev/null || true
  sleep 1
  if screencapture -x "$out" 2>/dev/null && [ -s "$out" ]; then
    echo "Screenshot saved: $out"
    return 0
  fi
  echo "Screenshot failed: $out"
  return 1
}
shot_ok=0
take_screenshot screenshot-macos-1.png && shot_ok=1
sleep 5
take_screenshot screenshot-macos-2.png && shot_ok=1
sleep 5
take_screenshot screenshot-macos-3.png && shot_ok=1

# Basic auth args for curl when a server password is configured (matches what
# the app itself does). In clean CI environments no password is set.
CURL_AUTH=()
if [ -n "${OPENCODE_SERVER_PASSWORD:-}" ]; then
  CURL_AUTH=(-u "${OPENCODE_SERVER_USERNAME:-opencode}:$OPENCODE_SERVER_PASSWORD")
fi

# --- poll for the opencode server --------------------------------------------
server_ok=0
end=$((SECONDS + TIMEOUT))
while [ "$SECONDS" -lt "$end" ]; do
  if ! app_alive; then
    echo "App process exited early (see opencode-web.log)."
    break
  fi
  body="$(curl -s --max-time 5 ${CURL_AUTH[@]+"${CURL_AUTH[@]}"} "http://127.0.0.1:$PORT/" || true)"
  if [ -n "$body" ]; then
    low="$(printf '%s' "$body" | tr '[:upper:]' '[:lower:]')"
    if printf '%s' "$low" | grep -qE 'opencode|healthy'; then
      server_ok=1
      echo "opencode server is responding on port $PORT."
      break
    fi
  fi
  sleep 3
done

proc_alive=0
if app_alive; then
  proc_alive=1
fi

ui_connected=0
if lsof -nP -iTCP:$PORT -sTCP:ESTABLISHED >/dev/null 2>&1; then
  ui_connected=1
fi

# --- verdict ------------------------------------------------------------------
if [ "$proc_alive" = "1" ] && [ "$server_ok" = "1" ]; then
  verdict=PASS
  detail="App stayed running and the opencode server responded on port $PORT."
elif [ "$server_ok" = "1" ]; then
  verdict=FAIL
  detail="opencode server responded on port $PORT, but the app process exited."
elif [ "$proc_alive" = "1" ]; then
  verdict=FAIL
  detail="App process is alive but the opencode server did not respond on port $PORT."
else
  verdict=FAIL
  detail="App process exited and the opencode server did not respond on port $PORT."
fi

# --- collect the app's opencode-web.log (the spawned opencode writes here) ----
OC_LOG=""
for c in "$HOME/Library/Logs/com.opencode.desktop/opencode-web.log" \
         "$HOME/Library/Application Support/com.opencode.desktop/logs/opencode-web.log"; do
  [ -f "$c" ] && OC_LOG="$c" && break
done
if [ -n "$OC_LOG" ]; then
  echo "opencode-web.log: $OC_LOG"
else
  echo "opencode-web.log: not found"
fi

{
  echo "## OpenCode Desktop verification report (macOS)"
  echo ""
  echo "- Installed app: $APP_PATH"
  echo "- App process running: $proc_alive"
  echo "- opencode server reachable (port $PORT): $server_ok"
  echo "- UI loaded (TCP connection to port $PORT): $ui_connected"
  echo "- Screenshots: screenshot-macos-1.png (t=0s), -2.png (t=5s), -3.png (t=10s)"
  echo "- Screenshot captured: $shot_ok"
  echo "- Verdict: **$verdict**"
  echo "- Detail: $detail"
  if [ -n "$OC_LOG" ]; then
    echo ""
    echo "### opencode-web.log (tail)"
    echo '```'
    tail -n 40 "$OC_LOG" 2>/dev/null || true
    echo '```'
  fi
} | tee report.txt

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  cat report.txt >> "$GITHUB_STEP_SUMMARY"
fi
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "verdict=$verdict" >> "$GITHUB_OUTPUT"
fi

if [ "$verdict" != "PASS" ]; then
  echo "::error::OpenCode Desktop verification FAILED on macOS."
  exit 1
fi
echo "::notice::OpenCode Desktop verification PASSED on macOS."
