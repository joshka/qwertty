#!/usr/bin/env bash
# Verify the live query path against real terminal implementations, headless.
#
# Runs the verify_queries smoke inside tmux and inside betamax (headless ghostty) when they are
# installed, skipping cleanly otherwise. Both type into the session while interleaved queries
# run, so the typeahead-survives-queries contract is exercised against real emulators.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --quiet --features tokio --example verify_queries

ran_any=0

if command -v tmux >/dev/null; then
    session=qwertty-verify
    tmux kill-session -t "$session" 2>/dev/null || true
    tmux new-session -d -s "$session" -x 100 -y 30
    tmux send-keys -t "$session" "cd $(pwd) && ./target/debug/examples/verify_queries; " 'echo VERIFY_EXIT=$?' Enter
    sleep 1.5
    tmux send-keys -t "$session" "hello"
    sleep 8
    pane=$(tmux capture-pane -t "$session" -p)
    tmux kill-session -t "$session"
    echo "$pane" | grep -E 'PASS|FAIL|Captured|VERIFY_EXIT' || true
    echo "$pane" | grep -q 'VERIFY_EXIT=0' || { echo 'tmux verification FAILED'; exit 1; }
    echo "$pane" | grep -q 'Captured "hello"' || { echo 'tmux typeahead FAILED'; exit 1; }
    echo 'tmux verification passed'
    ran_any=1
else
    echo 'tmux not installed; skipping'
fi

if command -v betamax >/dev/null; then
    QWERTTY_DIR=$(pwd) betamax run tapes/verify_queries.tape
    echo 'betamax verification passed'
    ran_any=1
else
    echo 'betamax not installed; skipping'
fi

if [ "$ran_any" -eq 0 ]; then
    echo 'no real-emulator tool available; nothing verified'
fi
