#!/bin/sh
# lakelands#848 probe: PROBE_848_N=<records> PROBE_848_LAYOUT=<one|one-last|two> PROBE_848=<explain|analyze> \
#   LAKELANDS_TEST_PG_URL=... cargo test --release -p lakelands-engine-pg --test probe_848 -- --ignored --nocapture
# Sum of the page reads: grep 'page .* read' | awk '{gsub(/ms|s/,"",$5)}...'
