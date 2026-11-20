# Pitfalls

Mistakes and upstream quirks documented as concrete rules so they are never repeated.

## Upstream APIs

- **HN Algolia rejects `points>N` in `numericFilters` with HTTP 400.** Only `created_at_i` is filterable server-side. Always overfetch and filter low-point stories client-side in `src/sources/hackernews.rs`.
- **Reddit anonymous `.json` endpoints return HTTP 403, and non-browser TLS clients may also get HTTP 403 on `/svc/shreddit/search/`.** Meanwhile Arctic Shift's `/api/posts/search?title=...` returns HTTP 400 unless `subreddit` or `author` is provided. `src/sources/reddit.rs` tries shreddit search first and falls back to Arctic Shift subreddit-prefix discovery (`/api/subreddits/search`) + subreddit post listing (`/api/posts/search?subreddit=...`).
- **arXiv quoted exact phrases miss multi-word natural queries.** Retry with `AND`-joined `all:"term"` clauses when the quoted phrase returns zero entries, and enforce a 365-day recency cutoff.

## MCP & CLI

- **Never write non-JSON logs to stdout in `radar30 mcp`.** All diagnostics go to stderr; stdout belongs exclusively to the MCP writer thread.
- **Never spawn a recursive MCP self-test inside `radar30_doctor` MCP tool.** CLI `radar30 doctor` spawns `<exe> mcp` for the live self-test; MCP `radar30_doctor` calls `doctor::run_without_mcp_selftest()`.
- **Keep `docs/tools.md` synchronized with `src/mcp.rs::tools()`.** `tests/mcp_contract.rs` fails if they drift. Regenerate with `RADAR30_REGEN_DOCS=1 cargo test --test mcp_contract`.

## BYOK Lanes

- **Never pass a secret to a CLI argument.** `radar30 creds set KEY --stdin` feeds `security -i` through stdin; `radar30 creds` only reports presence and origin.
- **Reddit/Bluesky public mirrors can answer HTTP 403 for non-browser TLS fingerprints.** Report an actionable `Unavailable`/`Forbidden` detail; do not retry aggressively or mask it as empty results.
- **YouTube flat playlist entries have no reliable publish date.** `date_confidence` is low and freshness scoring falls back to a neutral value; do not claim a 30-day cutoff for `yt-dlp` metadata.
- **X API recent search only covers about the last 7 days.** With `--days 30`, the X lane still returns recent tweets only; surface this in the preview of the lane, not as a 30-day guarantee.

## Company Intelligence

- **Yahoo Finance chart endpoints return HTTP 429 and Stooq serves a JS challenge here.** Use Nasdaq's keyless chart API; label `VOO` explicitly as an S&P 500 ETF proxy, not the index.
- **Greenhouse `content=true` can return 6 MB per board.** Request `content=false` (~0.5 MB) and stay under the HTTP response cap.
- **Google Patents XHR returns `&hellip;` and assignees in several shapes.** Decode entities and handle string/array/object assignees before rendering.
- **Do not call patent results "first US publications" without family data.** The digest reports US publication numbers and priority dates only.
- **Google Patents XHR can return HTTP 503 after repeated probes.** Fall back to the anonymous USPTO session (`/api/users/me/session` + `/api/searches/searchWithBeFamily`), then to a 24h cache; never synthesize an empty-but-successful patent section.

## macOS Packaging

- **APFS is case-insensitive by default.** A helper named `radar30` is the same file as the app executable `Radar30`; `swiftc -o .../Radar30` overwrote the bundled CLI. Name the helper `radar30-cli`.
- **`iconutil` refuses a directory without the `.iconset` extension.** Build the icons in `AppIcon.iconset`, then convert with `iconutil -c icns`.
- **A shell-script `CFBundleExecutable` using `osascript` can die silently when Finder launches it.** A native SwiftUI executable, or a `.command` handoff via LaunchServices, avoids the Automation prompt.
- **ScreenCaptureKit needs TCC approval.** When screenshot capture is denied, verify a native app through its accessibility tree and an invocation log instead of pixels.
