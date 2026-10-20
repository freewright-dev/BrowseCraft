#!/bin/bash
# Servo compatibility run (issue #65): opens each site in tools/servo-compat/sites.txt
# for 30 seconds in a Linux build with `--features servo`, and records its newest
# frame, title, load-complete time and any crash.
#
# Usage, from the repository root on Linux (a display is needed, e.g. WSLg):
#   cargo build --features servo
#   tools/servo-compat/run.sh [output-dir]      # default: target/servo-compat
set -u
cd "$(dirname "$0")/../.."
out=${1:-target/servo-compat}
bin=${BROWSECRAFT_BIN:-target/debug/browsecraft}
seconds=${SECONDS_PER_SITE:-30}
mkdir -p "$out"
printf "n\tsite\tframe\tcomplete_s\ttitle\tcrash\texit\n" > "$out/results.tsv"
n=0
while read -r site; do
  [ -z "$site" ] && continue
  n=$((n+1))
  id=$(printf "%02d" "$n")
  rm -f "$out/$id.png"
  BROWSECRAFT_SERVO_TRACE=1 BROWSECRAFT_SERVO_FRAME="$out/$id.png" \
    timeout "$seconds" "$bin" "https://$site" > "$out/$id.log" 2>&1
  code=$?
  [ -f "$out/$id.png" ] && frame=yes || frame=no
  # The first "complete" is the blank page the web view starts on.
  complete=$(grep "load Complete https\?://" "$out/$id.log" | tail -1 | awk '{print $2}')
  title=$(grep "servo-trace .* title " "$out/$id.log" | tail -1 | sed 's/^servo-trace [0-9.]*s title //' | tr '\t' ' ' | cut -c1-60)
  crash=$(grep -c "servo-trace .* crashed" "$out/$id.log")
  printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\n" "$n" "$site" "$frame" "$complete" "$title" "$crash" "$code" >> "$out/results.tsv"
  echo "$n $site frame=$frame complete=${complete:-none} crash=$crash exit=$code"
done < tools/servo-compat/sites.txt
