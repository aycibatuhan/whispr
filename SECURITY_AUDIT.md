# Whispr Fork — Security Notes

**Repo:** dbpprt/whispr (https://github.com/dbpprt/whispr) — fork with local LLM post-processing
**Audited:** 2026-08-11, local clone (includes the postprocess.rs addition)
**License:** MIT | **Author:** Dennis Bappert (dbpprt) | **Stars:** 29 | **History:** 31 commits, Dec 2024 – May 2026, active Dependabot

---

## 1. Files Reviewed

| File | Lines | Purpose |
|---|---|---|
| src-tauri/src/main.rs | 295 | App entry, hotkey flow, transcription → text insertion |
| src-tauri/src/audio.rs | 371 | Microphone capture (cpal), silence removal, resampling, WAV saving |
| src-tauri/src/config.rs | 194 | ~/.whispr/settings.json config with merge-on-load |
| src-tauri/src/hotkey.rs | 89 | Global/local NSEvent monitors (right ⌘ / right ⌥) |
| src-tauri/src/menu.rs | 588 | Tray menu: devices, language, translate, dev options |
| src-tauri/src/whisper.rs | 72 | whisper.cpp inference via whisper-rs (Metal) |
| src-tauri/src/window.rs | 105 | Transparent overlay window (waveform) |
| src-tauri/src/logging.rs | 103 | File + console logger to ~/.whispr/logs/ |
| src-tauri/src/postprocess.rs | ~90 | **OUR ADDITION** — Ollama client (localhost only) |
| src/App.tsx, main.tsx, App.css | 83+ | React waveform overlay, listens to status-change events |
| vite.config.ts, index.html, tsconfig*.json | — | Vite/TS config, dev server on localhost:1420 |
| src-tauri/Cargo.toml, Cargo.lock | — | Rust deps (all mainstream) |
| package.json, package-lock.json | — | React 19, Vite 6, @tauri-apps/api |
| tauri.conf.json, capabilities/default.json | — | Tauri v2 config; permissions: core, shell:allow-open only |
| Entitlements.plist, Info.plist | — | Microphone + audio-input entitlements only |
| build.rs, .github/dependabot.yml, .gitignore, .taurignore, .vscode/* | — | Standard scaffolding |

## 2. Hostname Map

| Host | Where | Purpose | Data sent |
|---|---|---|---|
| localhost:11434 | postprocess.rs:43,65 | **Ollama API** (our addition) | Transcription text (local only) |
| huggingface.co | config.rs:136 | Default model URL metadata (display only; download is manual per README) | None |
| localhost:1420 | tauri.conf.json | Vite dev server | None |
| github.com/dbpprt/whispr | main.rs:105, menu.rs:54 | Opens README in browser when model missing; About menu | None |

**No telemetry, analytics, tracking, or phone-home of any kind.** No Sentry/Amplitude/Mixpanel/gtag patterns found.

## 3. Credential Handling

- **Zero credentials in the codebase.** No API keys, tokens, passwords, or secrets anywhere.
- No `env::var` / `getenv` / `process.env` reads at all.
- Nothing is transmitted anywhere except localhost Ollama (our addition).

## 4. Code Execution

- Only two `.spawn()` calls, both `app.shell().command("open")` with **hardcoded** github.com URLs (opens the user's browser). No user input reaches a shell.
- No eval/exec/subprocess with dynamic input. No base64/obfuscated strings.

## 5. Filesystem Access

- Writes confined to `~/.whispr/`: settings.json, logs/, recordings/ (only when "Save Recordings" is enabled), model.bin.
- No access to ~/.ssh, ~/.config, system files, or other projects.

## 6. Dependencies

- **Rust:** tauri 2, whisper-rs (Metal), cpal, enigo, hound, serde, tokio, ureq (ours) — all well-known, actively maintained.
- **npm:** react, react-dom, @tauri-apps/api, vite, typescript — all mainstream.
- No suspicious/typosquatted packages. Dependabot active with weekly updates (31 commits, many dep bumps).

## 7. Code Quality / Red Flags

- Clean, readable, well-structured Rust. Single-purpose modules.
- Honest README: lists known issues (rough startup, static silence removal, menu flicker).
- Roadmap is transparent (MLX post-processing planned — which we're building).
- Minor: `Info.plist` has placeholder text "Your reasons here." for mic usage description (cosmetic, not security).
- Minor: `unsafe impl Send/Sync for AudioManager` (audio.rs:62-63) — common pattern for cpal wrappers, not a vulnerability.

## 8. Verdict: **SAFE** ✅ (primary) / **CAUTION** (independent review)

**Primary audit (this report): SAFE** — No data exfiltration, no credential handling, no exploitable code execution, no suspicious dependencies, filesystem access confined to its own directory. The app does exactly what the README claims: local push-to-talk transcription.

**Independent review: CAUTION** — no UNSAFE findings. Confirmed: no non-local transmission of audio/text/config/logs, no credential harvesting, no user-controlled command execution. Downgraded to CAUTION for these privacy-hygiene items (not vulnerabilities):

1. **Full transcriptions logged to disk by default** — `logging.rs` writes all log records (including the transcription text at main.rs:217-227 and whisper.rs:58-68) to `~/.whispr/logs/whispr_YYYYMMDD.log` when `developer.logging` is true (default).
2. **Recordings optionally persisted** — `save_recordings` writes raw WAV to `~/.whispr/recordings/` when enabled (default: off). Note: the `recordings_dir` config field exists but is unused — the path is hardcoded to `config_dir/recordings` (audio.rs:152-153), a minor upstream bug.
3. **Autostart writes a LaunchAgent** — the `auto-launch` crate creates `~/Library/LaunchAgents/...plist` when "Start at Login" is enabled (user-triggered, expected OS integration).
4. **Tauri CSP is null** — `tauri.conf.json` has `"csp": null` and global Tauri APIs are exposed to the webview (defense-in-depth only; the frontend is static and local, no active exploit path).
5. **argv/cwd logged** — the single-instance callback logs command-line arguments and cwd (main.rs:299-300). Only relevant if an external launcher passes a secret via argv.

**Reconciliation:** Both reviews agree there is no malicious behavior. The CAUTION items are privacy considerations for a voice app (transcripts on disk, optional WAV recordings), not security flaws. If you want stricter privacy: set `developer.logging: false` in `~/.whispr/settings.json` (or toggle Logging off in the tray menu) and keep `save_recordings` off.

**Caveat (trust tier, not code):** Solo maintainer, modest adoption — COMMUNITY-UNVERIFIED by adoption. The code is clean, but as with any small open-source project, the supply-chain risk lives in the dependency tree (mitigated here by Dependabot + mainstream crates). For a menubar app with mic access, this is acceptable — and this build pins the exact Cargo.lock.

**Our addition note:** postprocess.rs talks only to `localhost:11434` (Ollama). No transcription data leaves the machine. If you ever switch to a remote Ollama host, that would change this assessment.