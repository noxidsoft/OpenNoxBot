# To do

## Roadmap
Done: chat, engines and settings; approvals; guarded SSH; email connection (user-tested).
Guardrails come first: nothing runs unattended until approvals, limits and logs are solid.

5. **Usage and costs** (recording and Settings → Usage done, user-tested; spend limits, CSV export and provider allowances still to do): record cost and tokens per turn (the CLI already reports `total_cost_usd`), per-bot and per-day totals, Settings → Usage, spend limits that stop a bot. Everything below leans on this.
6. **Attachments and goals**: attach images and files to a message; a Goal button that keeps a bot working toward an outcome with turn, time and cost limits.
7. **Chief and workers**: delegation tools, task tree with depth/concurrency/cost limits, roles, Team Map view.
8. **Jev model selection**: route each task to a model by difficulty and cost (needs the Jev endpoint and the Decision and extraction settings).
9. **Automations and schedules**: cron with timezones, cheap code checks before any model turn, run history, pause/resume; headless-capable core.
10. **Logging in the UI**: one activity log across turns, tool calls, approvals, SSH, email and automation runs, filterable.
11. **Browser visuals and data view**: bots show pages and results visually; tables and charts for data.
12. **MCP connections (parked)**: generic stdio/http registry for Stripe, Xero, Google Sheets; research each one's sign-in first. Send-like tools always ask.
13. **Inbox bot**: see Later.

Reference screenshots for each area are in `~/Programs/screenshots-OpenNoxBot/` (usage, automations, team map, attach and goal buttons, decision and extraction, bot desktops, backup, import, remote access, servers).

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
- Inbox bot (user's stated goal): unattended, email tools only (no ssh/shell/web); per-bot "save drafts without asking" (never send, no send tool exists); scheduled triage that sorts mail and posts a daily digest of what needs their attention; label/move tools get their own approval setting, delete always asks
- Sandbox: block raw `ssh`/`scp` and network egress for bot shells (bubblewrap + seccomp)
- Run headless on a server, desktop as a client
- Cost tracking per bot and per run
- Windows and macOS builds (unix-only code in omb-ssh, omb-app and tests needs cfgs)
- Log email reads like ssh_log; per-folder limits
