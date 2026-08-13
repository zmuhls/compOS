# CompOS

Writing and reading software with one document authority. It runs first as ordinary
processes on a Mac or Linux machine; the immutable Debian appliance (amd64 UEFI, then
Raspberry Pi) is a deployment target, not the product.

**Status: Phases 0–2 complete, Phase 3 underway (2026-08-12).** `composd` core is
real: vault, content-addressed objects, append-only revision journal, the six-step
save transaction, SQLite/FTS5 rebuildable indexes, external-edit watcher, command
registry, and the UDS + WebSocket JSON-RPC boundary — gated by a kill −9 power-loss
torture test (1,000 iterations, 500-document vault, zero lost acknowledged writes),
an N/N-1 schema harness, the round-trip fixture law, and a constitution conformance
suite, all green in CI on macOS, Linux x86, and native arm64. The web shell ships
Write (CodeMirror 6), FTS5 Search, Markdown Read, and Review — keyboard-complete
and axe-core WCAG 2.2-gated in CI against a live daemon, touching the vault only
through composd. The Phase-3 proposal plane lives in composd: an agent-role
connection (capped at propose) opens line-hunk proposals, the reviewer accepts them
hunk by hunk, staleness is re-checked at accept time, and proposals survive
restart. The authoritative design is [ARCHITECTURE.md](ARCHITECTURE.md) (v2.0,
2026-08-09) — a vetted and resequenced revision of the original plan, with every
external claim fact-checked, the donor-code inventory verified against the
repositories on disk, and a corrections changelog in Appendix A.

Key decisions locked in v2.0:

- **All-purpose first:** the full product stack (composd + web shell) ships and is
  tested on macOS/Linux before any appliance image exists. Appliance work is Phases 6–7.
- **Rust** for `composd` and the service plane; TypeScript/React/CodeMirror 6 shell.
- **One API:** JSON-RPC 2.0 over UDS (services) and localhost WebSocket (shell), on
  every host profile; the command registry is the entire application surface.
- **Constitution enforced from day one** by in-process guards, file permissions, and a
  conformance test suite; systemd/EROFS hardening is additive at appliance time.

Next step: the rest of Phase 3 (ARCHITECTURE.md §16, §9) — `agentd` as the ACP
client host with provider adapters and the MCP broker, `modeld` for embeddings,
and prompt-injection hygiene at the agent boundary. The proposal plane they feed
is already live end-to-end.
