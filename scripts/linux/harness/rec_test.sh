#!/usr/bin/env bash
# rec_test.sh [--secs N] [--only A,B,C,D] [--no-crash] — recorder tests vs the LIVE app.
# See README.md. Exit 0 = all PASS, 1 = any FAIL.
exec python3 "$(dirname "${BASH_SOURCE[0]}")/rec_test.py" "$@"
