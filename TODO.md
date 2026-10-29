# To do

## Roadmap
1. ~~Chat, engine detection, settings~~
2. ~~Approvals: permission server, approval cards, own-folder reads~~
3. Guarded SSH: host registry, pinned host keys, per-host policy, audit log, `ssh_exec` tool
4. MCP connections: ~~custom email~~ (awaiting UAT), then Stripe, Xero, Google Sheets
5. Chief bot and role workers: delegation tools, task tree, depth/concurrency/cost limits, team map
6. Automations: cron with timezones, cheap checks, run history; headless-capable core

## Soon
- Desktop notification when a bot is waiting on an approval
- One card per action: merge the tool card and its approval card
- Stop button for a running turn
- Markdown rendering in replies
- Rename a bot; per-bot role and instructions
- Command allowlist approval level; "always allow" on an approval card
- AskUserQuestion answered in the UI (today it arrives as a plain approval)
- Extend the SSH status-command list from the person's own list of routine commands
- Jev Test button (needs the endpoint)
- Trust keys for hosts behind ProxyJump

## Later
- Sandbox: block raw `ssh`/`scp` and network egress for bot shells (bubblewrap + seccomp)
- Run headless on a server, desktop as a client
- Cost tracking per bot and per run
- Windows and macOS builds (unix-only code in omb-ssh, omb-app and tests needs cfgs)
- Log email reads like ssh_log; per-folder limits
