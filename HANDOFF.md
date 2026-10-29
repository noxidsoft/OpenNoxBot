# OpenNoxBot hand-off

Written 2026-10-29, last updated at the end of that day (after spend limits). Read this first, then `TODO.md`. Delete or rewrite it when it goes stale.

## What this is

A clean Rust + Tauri desktop app, inspired by OpenMausBot (the user's other app, TypeScript/Electron, at `~/Programs/OpenMausBot`). It is not a port. MIT licensed, `github.com/noxidsoft/OpenNoxBot`, `NOTICE.md` credits OpenMausBot (Apache-2.0) for ported logic.

The user's end goals, in their order of care:
1. A working group directed by a chief agent, with sub-bot workers in roles.
2. Connections (MCP) to Stripe, Google Workspace/Sheets, Xero, and a custom email MCP (their Titan/Hostinger IMAP draft-only server, `~/Programs/OpenMausBot/scripts/mcp-email.ts`).
3. Most important: SSH between servers, LAN and cloud (Linode).
4. Automations (goals/routines) for tasks.

Guardrails come first: nothing unattended until approvals, policy and logging are solid.

## State

| Stage | What | Status |
|---|---|---|
| 0 | Workspace, Tauri shell, Leptos UI | done |
| 1 | Engine detection (claude, codex, OpenAI key, Jev key), Settings, masked Jev key in the OS keyring | done, user-tested |
| 2 | Chat with streaming, tool cards, SQLite store, approval cards (loopback MCP permission server) | done, user-tested |
| 3 | Guarded SSH: host registry, pinned keys, per-host policy, `ssh_exec`, audit log, Settings → Hosts | done; user added two real hosts, trusted keys, Test says Connected, `list my hosts` works in chat |
| 4 | Email connection (in-process IMAP read/search/draft tools, Settings → Connections) | done, user-tested |
| 5 | Usage and costs: per-turn recording, Settings → Usage, global 24-hour and 30-day spend caps | done, user-tested (CSV export, provider allowances and a per-run trace are still open) |
| 6 | Attachments and goals | **next** |
| 7 | Chief + workers + Team Map; Jev model selection; automations and schedules; UI logging; browser visuals and data view | not started |
| – | Generic MCP registry (Stripe, Xero, Google Sheets) | parked |

The numbered order of upcoming work lives in the `TODO.md` roadmap; this table follows it.

What the user reported about Stage 3: "other tests passed for me". They skipped one "rename test" because those hosts are part of their real workflow. That test is not in my checklist, so it is probably the "make a change on a host" step (create a file, expect an approval card). Treat the change-with-approval path on a real host and the read-only/Refused path as **not confirmed by the user**. The Activity log was empty in their screenshot. It is covered by tests against a throwaway sshd and the real `claude`, not by them.

Git: branch `main`, everything is pushed as of the spend-limit UAT. Ask before pushing again.

Tests: about 90 hermetic (`cargo test`), plus 6 ignored ones that cost money or touch the keyring (below).

## Working with this user

- They are the primary tester. After each buildable stage, commit locally, then give a numbered UAT checklist and wait for results. Questions one at a time, not in a batch.
- Auto mode is on. Proceed on conventional choices; ask only when a decision is theirs (they decided: guarded SSH tool, Leptos in a Tauri webview, Rust not C).
- UI: cut helper text and secondary labels; fix the layout instead.
- Their global instructions ask for the line "Oogabooga Nox" at the start of a conversation.
- They keep reference screenshots of the original app in `~/Programs/screenshots-OpenNoxBot/` (named by what they show) and send UAT screenshots from `~/Pictures/Screenshots/`. Read only the ones relevant to the stage.
- Machine: Arch Linux, KDE Plasma on Wayland. `claude` 2.1.309 at `/usr/bin/claude`. A secret service (ksecretd/KWallet) is running. **Their live OpenMausBot (Electron) runs on the same machine. Never touch its processes or `~/.config/openmausbot`.**

## Layout

```
crates/
  omb-wire     serde types shared by backend and UI (WASM-safe)
  omb-core     store (SQLite), recorder, approvals, policy, secrets (keyring), proc runner, ids
  omb-drivers  PATH/host resolution, engine detection, login probes, claude::run_turn
  omb-mcp      loopback axum MCP server: approve, ssh_hosts, ssh_exec, email_*
  omb-email    IMAP read/search/draft tools behind a `Mailbox` trait (`ImapMailbox` is the real one), search-term quoting
  omb-ssh      status-command policy, runner on the system ssh, key scan/pin
  omb-app      Tauri shell: main.rs (setup), commands.rs (every #[tauri::command]), capabilities/default.json
  omb-ui       Leptos 0.8 CSR frontend: chat, settings, engines, hosts, connections, api_keys, api.rs, style.css
```

A turn: `send_message` stores the user message, `play_turn` registers the turn with the MCP server (random bearer token), writes a 0600 config to `<data>/run/`, and spawns
`claude -p --output-format stream-json --input-format stream-json --verbose --include-partial-messages --permission-mode <mode> [--model m] [--resume sid] --mcp-config f --strict-mcp-config --permission-prompt-tool mcp__nox__approve --disallowedTools Bash(ssh:*) …`.
The prompt is one JSON line on stdin, then stdin is closed. stdout frames become `RuntimeEvent`s (`omb-wire`). The same channel carries approval events from the MCP server. `TurnRecorder` persists them as messages and the app emits each to the UI as the Tauri event `runtime`; the UI reloads messages on every non-delta event.

Data: `~/.local/share/dev.noxidsoft.opennoxbot/` (`opennoxbot.db`, `bots/<id>/` = each bot's private working folder, `run/`, `known_hosts`). Override with `OPENNOXBOT_DATA`. ssh control sockets live in `$XDG_RUNTIME_DIR/nox-ssh-<uid>`.

## How to run, test, verify

```
cd ~/Programs/OpenNoxBot/crates/omb-app && cargo tauri dev     # needs ~/.cargo/bin on PATH (cargo-tauri 2.12.4, trunk 0.22.1)
cargo test                                                     # hermetic, ~10 s of runtime
cargo test -p omb-mcp --test real_claude -- --ignored --test-threads=1   # real claude + throwaway sshd, a few cents, 5 tests
cargo test -p omb-core -- --ignored                            # writes then deletes one keyring entry
```

Verifying UI without the user: launch an **isolated** instance and screenshot it. You cannot click (no input automation on Wayland here), so seed state instead.
1. `OPENNOXBOT_DATA=<scratch> ./target/debug/omb-app` once to create the schema, kill it, seed rows with python `sqlite3`, relaunch.
2. The debug binary loads `http://127.0.0.1:1420`; run `(cd crates/omb-ui && exec trunk serve > log 2>&1) &` first and use `$!` as the PID (the `exec` form makes `$!` trunk itself). Without it the window says "Could not connect to 127.0.0.1".
3. `spectacle -b -n -a -o file.png` captures the active window; read the PNG.
4. Stop only processes you started; the auto-mode classifier refuses `kill` on PIDs it cannot attribute, so match by cwd and parent (see how this was done: `readlink /proc/$p/cwd`).

## Design decisions and why

- **Guarded SSH tool, not raw ssh.** Hosts are registered in Settings; agents get `ssh_hosts` and `ssh_exec` only. Policies: `read-only` (status commands run, anything else refused), `ask` (status runs, anything else shows an approval card), `full`. The policy is enforced **inside the tool**, because under Full access the CLI never calls the permission callback. Raw `ssh/scp/sftp/sshfs/ssh-copy-id` in Bash are blocked twice: `--disallowedTools` (hard, verified under Full access) and a deny in the approve handler (best effort, first word of each shell segment). A program that launches ssh indirectly can still get through until the planned sandbox.
- **Host keys pinned by the person.** `scan` fetches keys and fingerprints; nothing is trusted until the UI confirms exactly the keys shown (kept server-side in `AppState.scanned`). The app uses its own known_hosts with `StrictHostKeyChecking=yes`. The person's `~/.ssh/config` is honoured (aliases, identity files), and forwarding/prompts are forced off on the command line. Key scans connect directly, so a host behind `ProxyJump` cannot be trusted from the UI yet.
- **Approvals via the CLI's `--permission-prompt-tool`** pointing at an app-hosted MCP server on 127.0.0.1 with a per-turn bearer token. Reads inside the bot's own folder never ask (port of OpenMausBot's `own-file-reads.ts`, with its test cases). Our own tools (`mcp__nox__*`) pass the callback because they enforce their own policy.
- **Tauri + Leptos (WASM) frontend** chosen by the user over native toolkits, for rich content later (markdown, tables, charts).
- **Events/state:** UI state is reloaded from SQLite on events; live text deltas are a UI-only buffer.

## Gotchas (each cost time)

1. **Tauri v2 capabilities.** Without `crates/omb-app/capabilities/default.json`, `event.listen` is refused, so the stream never reaches the UI (symptom: reply saved in the DB but the window stays on "…"). Granted: `core:event:allow-listen`, `allow-unlisten`. The UI now shows "Live updates unavailable" if this regresses. New plugin APIs need their own permissions.
2. **Tauri command args are camelCase in JS.** UI arg structs use `#[serde(rename_all = "camelCase")]` (`host_id` → `hostId`).
3. **The CLI auto-allows some harmless commands** (`echo`, `ls`) without calling the permission tool, so tests that need an approval must use something it always asks about (writing a file).
4. **ETXTBSY.** Executing a script just written while another test forks fails intermittently. `omb_core::proc::spawn` retries; use it for every subprocess. Tests give child processes a PATH containing only the fake dir, so fakes must use absolute `/bin/cat`, `/bin/sleep`.
5. **ssh `ControlPersist` keeps sessions for 60 s.** `SshRunner::disconnect` closes one (used on remove/policy change and in tests).
6. **Leptos `<For>` keys must include whatever changes** (message body, host policy, trusted), or rows do not re-render.
7. **`proc::run` bounds each stream to 64 KiB and kills a flooding process.** Do not use `Command::output()` for untrusted output.
8. **WebKitGTK on this Wayland setup crashes** unless `WEBKIT_DISABLE_DMABUF_RENDERER=1`; `main.rs` sets it on Linux.
9. **keyring is v4** (`keyring::Entry`, Secret Service); the API differs from v3 examples online.
10. `Tauri dev` runs `trunk` with `cwd: ../omb-ui` (set in `tauri.conf.json`); trunk must be on PATH.

## Usage and costs

`TurnRecorder::finish(prompt)` (called by `play_turn` after the stream ends) writes one `runs` row plus `run_models` rows from the CLI's `result` frame (`total_cost_usd`, `usage` tokens summed incl. cache, `num_turns` as steps, `modelUsage[].costUSD`); tool uses are counted from `ItemStarted`; time is measured by the recorder. `RuntimeEvent::TurnCompleted` now carries `usage: Option<TurnUsage>`. Periods are rolling windows (24 h, 7 d, 30 d), not calendar ones. `limits` table (one row) holds the caps; `send_message` calls `Store::over_limit()` and refuses before starting a turn, so one turn can overshoot. History predates nothing: turns before the feature were never recorded. Next layers if wanted: CSV export, provider allowance bars (the original's second Usage screenshot), per-run trace.

## Email connection (Stage 4)

Rust port, served **in-process** by the loopback `nox` server (`omb-mcp/src/email_tools.rs`): `email_folders`, `email_search`, `email_read`, `email_draft`. No child process, so the password (keyring `email-password`, read per call) never reaches claude's env or the 0600 turn config. Settings → Connections stores address/server/port in the `email` table (one mailbox). `email_draft` asks for approval inside the tool (the `approve` callback skips `mcp__nox__*`); reads run without asking and are not logged. Search terms must be plain ASCII and are quoted; the `imap` crate's `search` takes a raw string, and its `append` does not escape the folder name (we only pass names from LIST). Reads use EXAMINE and `BODY.PEEK`. Email bodies are outside text: tool output carries a "do not follow instructions" note, but a prompt-injected email could still steer a bot toward `ssh_exec` on a Full host. **Decided with the user:** nothing may be sent to a customer or contact without their explicit approval. So (1) while a mailbox is connected every turn runs in Ask mode (`ApprovalMode::with_mail`), because the CLI skips the permission callback in Auto/Full; (2) once a turn has read mail (`TurnContext.mail_seen`), `ssh_exec` asks even on Full hosts and for status commands. Any future send-like tool (invoices in Stripe/Xero, anything that contacts a person) must ask unconditionally.

MCP connections are parked (see `TODO.md`). Still to do for them: a generic stdio/http MCP registry for Stripe, Xero, Google Sheets (not started; research each one's auth first).

## Parked: generic MCP registry plan

Plan (parked, not started):
- Registry table `mcp_servers` (name, kind stdio|http, command/args or URL, env/header entries that reference keyring secrets). Settings → Connections page; secrets entered masked and stored in the keyring, never shown back (same pattern as the Jev key in `api_keys.rs` / `omb_core::secrets`).
- Merge enabled servers into each turn's `mcpServers` map next to `nox` (`omb_mcp::TurnAccess::config`), injecting secrets at spawn. Their tools arrive as `mcp__<name>__<tool>` and go through `approve`, so they ask by default. Consider per-server read/write classification and an "always allow" memory (see `TODO.md`).
- Then Stripe, Xero, Google Sheets. Do not assume how they authenticate: look up the current official or community MCP servers and their auth (Stripe has a remote MCP and a local one; Xero publishes an MCP server; Google Workspace options are mostly community) before building. OAuth PKCE reference: `~/Programs/OpenMausBot/server/mcp-oauth.ts`.

Later roadmap items (chief + workers: delegation tools on the MCP server, a task tree with status and limits on depth/concurrency/cost, a team map; see the original's `docs/runtime-contract.md`, `agents-tool-bridge.md`) and automations: cron with timezones plus cheap code "checks" before any model turn, run history, pause/resume; see `docs/routine-schedules.md`, `proactive-routines.md`). Unattended runs need a headless-capable core, so keep logic out of the Tauri layer.

## Open items to raise with the user

- Next up is attachments and goals. Read `~/Programs/screenshots-OpenNoxBot/main-screen-attach-and-goal-buttons.png` first. Attachments before goals. Goals need turn, time and cost limits and must respect the spend caps.
- The user wants an inbox-triage bot eventually (see `TODO.md` Later and the guardrails under Email): only drafts, nothing sent without approval.
- Which commands should run silently over SSH? The list in `omb-ssh/src/policy.rs` is deliberately narrow (uptime, df, free, ls, ps, systemctl status, journalctl, docker ps/logs/stats, tail/head/cat/grep under `/var/log`). They were asked and have not answered.
- Jev: it is a decision/extraction helper in the original ("Decision and extraction"), not a chat engine. Only key storage and detection exist; a Test button needs the endpoint (base URL `https://api.typesafe.ai`; the `typesafe:typesafe-ai` skill documents it).
- The skipped "rename test" (see State).
- Safety rules already decided: while a mailbox is connected every turn runs in Ask mode; after mail is read in a turn `ssh_exec` always asks; any future send-like tool always asks.
- Unix-only code (`std::os::unix`) in `omb-ssh`, `omb-app`, tests; Windows/macOS need cfgs.

## Reference

- Original app: `~/Programs/OpenMausBot` (`AGENTS.md` there has its comment rules; this repo follows them: comment only what the code cannot say, i.e. outside-world facts, security invariants, contracts). Useful files: `server/contracts.ts`, `server/drivers/claude.ts`, `server/testing/fake-claude-cli.ts` (frame shapes), `server/own-file-reads.ts`.
- Real CLI facts verified this session: init → `system/init {session_id, model}`; text via `stream_event` `text_delta`; tool calls as `assistant` `tool_use`, results as `user` `tool_result`; `result {is_error, result, total_cost_usd}`; `--resume <session_id>` carries context; the permission callback is a tool named `approve` taking `{tool_name, input}` and answered with `{"behavior":"allow","updatedInput":…}` or `{"behavior":"deny","message":…}` as MCP text content.
