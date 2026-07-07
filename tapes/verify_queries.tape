Output /tmp/qwertty-verify-betamax.json
Set Shell "bash"
Set Width 1000
Set Height 700
Type "cd \"$QWERTTY_DIR\" && cargo run --quiet --example verify_queries --features tokio 2>&1"
Enter
Wait+Screen@120s "letters NOW"
Type "hello"
Wait+Screen@20s "Captured \"hello\""
Wait+Screen@20s "Self-checked: 2 pass, 0 fail"
