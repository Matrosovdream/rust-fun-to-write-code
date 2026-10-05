# Progress — net-protocols

Status legend: `—` not started · `ref` reference written · `✍️` rewriting · `✅` rewritten & compared

Rewrites go in `practice/other/net-protocols/<name>`.

| # | Project | Group | Port | Reference | My rewrite | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | linechat | A | 7000 | ref | — | |
| 2 | smtpd | A | 2525 | ref | — | |
| 3 | miniredis | A | 6399 | ref | — | |
| 4 | binkv | A | 7100 | ref | — | |
| 5 | jsonrpc | A | 7200 | ref | — | |
| 6 | udpping | B | 7300/udp | ref | — | |
| 7 | tftpd | B | 6969/udp | ref | — | |
| 8 | dnsd | B | 10053/udp | ref | — | |
| 9 | httpd | C | 8180 | ref | — | |
| 10 | fetchr | C | — | ref | — | |
| 11 | restapi | C | 8181 | ref | — | |
| 12 | ssefeed | C | 8182 | ref | — | |
| 13 | websock | C | 8183 | ref | — | |
| 14 | httpproxy | D | 8888 | ref | — | |
| 15 | loadbal | D | 9400 | ref | — | |
| 16 | asyncchat | E | 7001 | ref | — | |
| 17 | tunnel | E | 7500/8090 | ref | — | |

## Activity log

- 2026-10-05 — plan created; all 17 reference implementations written
- 2026-10-05 — `cargo test --workspace` green (244 tests), clippy `-D warnings` clean;
  every server smoke-tested on its default port with the real tool (`nc`, `curl`,
  `dig`, macOS `tftp`, raw WebSocket client). Ports moved off 6380/8080/8082/9000/9001,
  which local Docker containers hold.
