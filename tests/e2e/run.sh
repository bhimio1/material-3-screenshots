#!/usr/bin/env bash
# End-to-end tests for the m3-screenshots command line, against stub grim,
# slurp, wl-copy, hyprctl, hyprpicker and recorders (tests/e2e/bin) and a
# fake notification server, in a private D-Bus session. Needs Python 3 with
# dbus-python and PyGObject, and ImageMagick.
#
#   tests/e2e/run.sh [path/to/m3-screenshots]
set -u
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-$here/../../target/debug/m3-screenshots}
bin=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")
py=${PYTHON:-python3}

if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ] || [ -z "${M3S_E2E_INNER:-}" ]; then
    exec env M3S_E2E_INNER=1 dbus-run-session -- "$0" "$bin"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export HOME=$work/home XDG_CONFIG_HOME=$work/home/.config XDG_CACHE_HOME=$work/home/.cache
export XDG_RUNTIME_DIR=$work/run STUB_DIR=$work/stub
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" "$STUB_DIR" "$XDG_CONFIG_HOME/m3-screenshots"
chmod 700 "$XDG_RUNTIME_DIR"
export PATH=$here/bin:$PATH
shots=$work/shots
cat > "$XDG_CONFIG_HOME/m3-screenshots/config.toml" <<TOML
save_dir = "$shots"
filename_pattern = "{mode}-{window}"
shutter_sound = true
[recorder]
save_dir = "$work/videos"
filename_pattern = "rec-{mode}"
backend = "gpu-screen-recorder"
TOML

pass=0 fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
size() { identify -format "%w %h" "$1" 2>/dev/null; }
# R and G of a pixel (they encode the logical position, see bin/grim).
px() { convert "$1" -format "%[fx:int(255*p{$2,$3}.r+0.5)] %[fx:int(255*p{$2,$3}.g+0.5)]" info: 2>/dev/null; }

notifyd_pid=
notifyd() {
    [ -n "$notifyd_pid" ] && kill "$notifyd_pid" 2>/dev/null && wait "$notifyd_pid" 2>/dev/null
    FAKE_ACTION=$1 "$py" "$here/fake-notifyd.py" > "$work/notifyd.out" 2>&1 &
    notifyd_pid=$!
    for _ in $(seq 50); do grep -q ready "$work/notifyd.out" 2>/dev/null && return; sleep 0.1; done
    echo "fake notification server didn't start:"; cat "$work/notifyd.out"; exit 1
}
reset() { rm -f "$STUB_DIR"/*; rm -rf "$shots"; }

echo "1. region, frozen, saved and copied"
reset; notifyd close
SLURP_ANSWER="100,50 200x100 kitty" "$bin" region --save --copy > "$work/out" 2>&1
code=$?
f=$shots/region-kitty.png
check "exit 0" "[ $code = 0 ]"
check "prints the path" "grep -qx '$f' '$work/out'"
check "saved at 2x (400x200)" "[ \"\$(size '$f')\" = '400 200' ]"
check "crop starts at logical 100,50" "[ \"\$(px '$f' 0 0)\" = '100 50' ]"
check "crop ends at logical 299,149" "[ \"\$(px '$f' 399 199)\" = '43 149' ]"
check "grim grabbed the whole layout first" "grep -qx 'grim -t ppm -' '$STUB_DIR/calls.log'"
check "hyprpicker froze the screen" "grep -qx 'hyprpicker -r -z' '$STUB_DIR/calls.log'"
check "hyprpicker is gone" "! pgrep -fx 'sleep 30' >/dev/null"
check "slurp got the window boxes" "grep -q '^10,20 300x200 kitty' '$STUB_DIR/slurp-stdin.txt'"
check "slurp didn't get windows from hidden workspaces" "! grep -q hidden-ws '$STUB_DIR/slurp-stdin.txt'"
check "slurp themed" "grep -q -- '-c #' '$STUB_DIR/calls.log'"
check "copied as PNG" "grep -qx 'wl-copy --type image/png' '$STUB_DIR/calls.log' && [ \"\$(size '$STUB_DIR/clipboard.bin')\" = '400 200' ]"
check "shutter sound" "grep -q 'pw-play' '$STUB_DIR/calls.log' || [ ! -e /usr/share/sounds/freedesktop/stereo/camera-shutter.oga ]"
check "notified with actions" "grep -q '\"summary\": \"Screenshot saved and copied\"' '$STUB_DIR/notifications.log' && grep -q '\"edit\", \"folder\", \"delete\"' '$STUB_DIR/notifications.log'"
check "notification shows the image" "grep -q '\"image\": \"$f\"' '$STUB_DIR/notifications.log'"

echo "2. focused output, not frozen, defaults from the config"
reset; notifyd close
"$bin" output --active > "$work/out" 2>&1
f=$shots/output-eDP-1.png
check "grim -o eDP-1" "grep -qx 'grim -t ppm -o eDP-1 -' '$STUB_DIR/calls.log'"
check "native size 1200x800" "[ \"\$(size '$f')\" = '1200 800' ]"
check "no slurp, no freeze" "! grep -q '^slurp\|^hyprpicker' '$STUB_DIR/calls.log'"
check "copied (config default)" "[ -s '$STUB_DIR/clipboard.bin' ]"

echo "3. pick a window; the Delete button deletes it"
reset; notifyd delete
SLURP_ANSWER="650,30 400x300 firefox" "$bin" window --no-freeze > "$work/out" 2>&1
f=$shots/window-firefox.png
check "slurp restricted to windows" "grep -q '^slurp .* -r$' '$STUB_DIR/calls.log'"
check "grim -g on the window" "grep -qx 'grim -t ppm -g 650,30 400x300 -' '$STUB_DIR/calls.log'"
check "deleted from the notification" "[ ! -e '$f' ] && grep -q '\"closed\"' '$STUB_DIR/notifications.log'"

echo "4. pick an output, frozen: back to its native pixels"
reset; notifyd close
SLURP_ANSWER="600,0 800x600 " "$bin" output > "$work/out" 2>&1
f=$shots/output-DP-1.png
check "slurp -o -r" "grep -q '^slurp .* -o -r$' '$STUB_DIR/calls.log'"
check "DP-1 at its own 800x600" "[ \"\$(size '$f')\" = '800 600' ]"
check "DP-1 pixel 10,10 is logical 610,10" "[ \"\$(px '$f' 10 10)\" = '98 10' ]"

echo "5. cancelled selection"
reset; notifyd close
SLURP_ANSWER=cancel "$bin" region > "$work/out" 2>&1
code=$?
check "exit 1" "[ $code = 1 ]"
check "nothing saved or notified" "[ ! -d '$shots' ] && [ ! -s '$STUB_DIR/notifications.log' ]"
check "the screen was frozen during selection" "grep -qx 'hyprpicker -r -z' '$STUB_DIR/calls.log'"

echo "6. everything, copy only, unsaved copy kept for History"
reset; notifyd close
"$bin" all --copy > "$work/out" 2>&1
check "2800x1200 on the clipboard" "[ \"\$(size '$STUB_DIR/clipboard.bin')\" = '2800 1200' ]"
check "not saved to the folder" "[ ! -d '$shots' ]"
check "stashed in the cache" "ls '$XDG_CACHE_HOME'/m3-screenshots/unsaved/*.png >/dev/null 2>&1"
check "notification offers Save" "grep -q '\"save\"' '$STUB_DIR/notifications.log'"

echo "7. no notification with --no-notify; delay"
reset; notifyd close
start=$(date +%s)
"$bin" all --save --no-notify --delay 1 > "$work/out" 2>&1
check "waited a second" "[ \$(( \$(date +%s) - start )) -ge 1 ]"
check "no notification" "[ ! -s '$STUB_DIR/notifications.log' ]"

echo "8. recording with gpu-screen-recorder, toggled"
reset; notifyd close
SLURP_ANSWER="100,100 320x240 " "$bin" record region --audio > "$work/out" 2>&1
code=$?
check "started" "[ $code = 0 ] && [ -f '$XDG_RUNTIME_DIR/m3-screenshots/recording.json' ]"
check "gsr args" "grep -qx 'gpu-screen-recorder -w region -region 320x240+100+100 -f 60 -q high -c mp4 -cursor yes -a default_output -o $work/videos/rec-region.mp4' '$STUB_DIR/calls.log'"
check "status --json says recording" "'$bin' status --json | grep -q '\"recording\":true'"
check "a second start refuses" "! SLURP_ANSWER='0,0 10x10 ' '$bin' record region"
"$bin" record region --toggle > "$work/out" 2>&1
check "toggle stopped it" "[ ! -f '$XDG_RUNTIME_DIR/m3-screenshots/recording.json' ] && [ -s '$work/videos/rec-region.mp4' ]"
sleep 0.5
check "status idle" "'$bin' status --json | grep -q '\"recording\":false'"
check "Recording saved notification" "grep -q 'Recording saved' '$STUB_DIR/notifications.log'"

echo "9. recording with wf-recorder, focused output, record stop"
reset; notifyd close
sed -i 's/backend = "gpu-screen-recorder"/backend = "wf-recorder"/' "$XDG_CONFIG_HOME/m3-screenshots/config.toml"
"$bin" record output --active --audio > "$work/out" 2>&1
check "wf-recorder args" "grep -qx 'wf-recorder -o eDP-1 -r 60 -p crf=23 --audio=alsa_output.test.monitor -f $work/videos/rec-output.mp4' '$STUB_DIR/calls.log'"
"$bin" record stop > "$work/out" 2>&1
check "stopped and saved" "grep -q rec-output.mp4 '$work/out' && [ -s '$work/videos/rec-output.mp4' ]"
check "nothing left to stop" "! '$bin' record stop 2>/dev/null"

echo "10. usage errors"
check "unknown command exits 2" "'$bin' selfie >/dev/null 2>&1; [ \$? = 2 ]"
check "--help" "'$bin' --help | grep -q 'record region|window|output'"

[ -n "$notifyd_pid" ] && kill "$notifyd_pid" 2>/dev/null
echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
