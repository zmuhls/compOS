# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CompOS is writing/reading software with one document authority (`composd`). **ARCHITECTURE.md is the design authority** — read the relevant section before changing anything structural; section numbers below refer to it. The build plan (§16) runs Phase 0 → 7; Phases 0–2 are complete (vault, objects, journal, save transaction, SQLite/FTS5 tier-2, external-edit watcher, command registry, compos-rpc, N/N-1 harness, round-trip harness, conformance suite, torture gates, and the web shell — Write/Read/Search/Review, palette, WCAG 2.2 axe + keyboard-complete e2e gates; the `journal.rs` and `lease.rs` `DECISION(user)` markers were ratified 2026-08-11). Phase 3's composd-side proposal plane is live (store, per-hunk accept, accept-time stale recheck, conformance gates; the proposal record format carries an open `DECISION(user)` marker in `proposal.rs`). Remaining Phase 3: `agentd` (ACP client host, MCP broker), `modeld` (embeddings), prompt-injection hygiene.

## Commands

```sh
cargo fmt --all --check                 # CI gate; run `cargo fmt --all` to fix
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # includes torture at 100 iterations
cargo test -p compos-core --test reconcile_windows   # one integration suite
cargo test -p compos-core save_and_read_back         # one test by name
TORTURE_ITERS=1000 cargo test -p composctl --test torture   # milestone gate
TORTURE_ITERS=1000 TORTURE_DOCS=500 cargo test -p composctl --test torture  # phase-1 exit gate
cargo test -p compos-rpc --test conformance          # constitution suite
cargo test -p compos-core --test proposals           # proposal plane suite
cargo test -p composd --test daemon                  # live daemon + watcher e2e (notify-timing sensitive)
pnpm install && pnpm -C shell typecheck && pnpm -C shell build   # web shell
cargo build -p composd -p composctl && pnpm -C shell test:e2e    # playwright + axe WCAG 2.2 gate (boots a real composd)
cargo run -p composctl -- --vault /tmp/v init        # manual smoke
cargo run -p composd -- --vault /tmp/v               # run the daemon (uds + ws :7411)
```

`composctl` subcommands: `init save log cat search scan version` (+ hidden `_torture-child`). `composd` flags: `--vault --ws-port --socket --self-check`; it writes the WebSocket auth token to `<vault>/state/rpc-token` (0600) at startup.

CI (`.github/workflows/ci.yml`) runs the rust matrix on macos-latest, ubuntu-latest, and ubuntu-24.04-arm (native arm64) plus the shell job. All must stay green — the arm runner exists so Pi problems surface years early.

Commits: short, lowercase, no sign-off.

## Architecture (the parts that span files)

Four crates: `compos-core` (pure sync library — all the semantics), `compos-rpc` (tokio UDS + WebSocket JSON-RPC boundary; role typing and capability caps live here, no document logic), `composd` (daemon: opens the vault, serves compos-rpc, runs the notify watcher), `composctl` (CLI client linking compos-core directly; also hosts the hidden `_torture-child` subcommand). Keep the async boundary where it is: **compos-core stays synchronous** — tokio appears only in compos-rpc and composd, and deliberate absences stay absent (no chrono: integer-ms timestamps; no JSON-RPC framework: `proto.rs`/`session.rs` are the whole protocol).

**The journal is truth.** `journal/` holds append-only JSONL records (canonical tier-1 state, §5.2); document heads are *derived* by replay in `DocIndex` — there is deliberately no pointer file and no second source of truth. `state/compos.db` (`derived.rs`) is a rebuildable FTS5 cache fed by the same replay: its **only** migration policy is destroy-and-rebuild on any `user_version` skew, and `tests/derived_rebuild.rs` proves delete → rebuild → identical results (rule 4). Anything that would create a second authority is a constitution violation.

**The save transaction** (`writer.rs`, §5.3) is six steps with exact fsync ordering: verify base+lease → object put → durable write-intent → atomic visible-file replace → journal append+fsync (**the acknowledgment point** — nothing is acked before this) → intent cleanup + derived-index feed (post-ack: a derived failure warns, never fails the save). `reconcile.rs` classifies every crash window (W1–W7 plus the external-edit edge); `tests/reconcile_windows.rs` hand-builds each window deterministically. If you touch the save path or reconciliation, the torture test (`crates/composctl/tests/torture.rs`) is the gate. Run it at 1000 iterations (and once with `TORTURE_DOCS=500`) before calling such a change done.

**Never clobber user bytes — enforced twice.** The save transaction itself has a guard: if the visible file matches neither the recorded head nor the incoming content, those bytes are committed as an `external` revision first and the save fails `StaleBase` (tests in `external_scan.rs`). `Vault::scan_external` sweeps the whole vault the same way; composd's notify watcher calls it on filesystem events. Hidden files (dot-prefixed components) are ignored by the scan; file deletion is report-only (no delete semantics in the Phase-1 model).

**Single-writer enforcement is by construction** (rule 1, §3): `VaultWriter` is the only mutation path, obtainable only from `Vault::open_write`, which holds an exclusive flock; a second writer gets `VaultBusy`. `VaultError` variants mirror the RPC wire errors 1:1 — `proto.rs::map_vault_error` is the single mapping; extend both sides together when adding variants.

**The registry is the API** (§7, `command.rs`): the RPC surface is six fixed methods, and everything else is a registry command with an effect class (`read < propose < commit < system`). Inputs are validated against the same JSON Schema `commands.describe` publishes. Role caps at the boundary (`session.rs`): shell/service ≤ commit, agent ≤ propose, maintenance ≤ system — an agent invoking a commit command gets `CAPABILITY_DENIED` before dispatch (rule 6; `tests/conformance.rs` is the constitution suite and must keep passing as-is).

**Proposals are durable composd state** (§5.2, §9, `proposal.rs`): `journal/proposals/*.jsonl` is an append-only create/resolve event log with the same tail-repair and fsync discipline as the revision journal, deliberately placed in a subdirectory so format-1 revision replay never sees it (no `vault_format` bump; N and N-1 keep opening each other's vaults). State is derived by replay; staleness is always computed, never stored. `proposal.accept.hunk` re-verifies the base at accept time and commits through the ordinary save transaction as a `proposal-accept` revision — canonical state moves first, the resolve record second, so a crash between them leaves an open-and-stale proposal, never a false acceptance. Effect classes carry rule 6: `create`/`withdraw` are propose, `accept.hunk`/`reject` are commit. The record wire format and placement carry an open `DECISION(user)` marker in `proposal.rs` — working defaults shipped, not yet ratified.

**The shell holds no canonical state** (`shell/src`): one WebSocket JSON-RPC client (`rpc.ts`, token presented at `hello`), four modes plus a `commands.list`-driven palette, and event-driven refresh (`revision.committed`, `proposal.updated`/`proposal.stale`; sequence gaps trigger resync). The e2e suite (`shell/tests`) boots a real composd over a scratch vault in `global-setup.ts` — keep it mock-free; the axe WCAG 2.2 sweep and the keyboard-only write/save/search loop are the Phase-2 CI gates (`shell-e2e` job).

**Frozen fixtures are contracts.** `compos-core/tests/fixtures/vault-format-N/` are golden vaults generated once by the build that introduced format N and never regenerated — `schema_compat.rs` forces every supported format to keep opening (tier-1 N/N-1 gate). `tests/fixtures/roundtrip/<codec-id>/` is the round-trip corpus (`codec.rs`; the `Codec` trait makes one-way codecs unrepresentable).

**Ratified contracts (2026-08-11, owner decision — do not revisit unilaterally):** the `JournalRecord` wire format is final for vault format 1 (short field names, `path` in-record, integer-ms `ts`, three-tier bump policy — details in `journal.rs`); leases are optional/advisory over base-matching, 60 s sliding TTL renewed by the holder's saves or explicit renew, auto-release on expiry, no steal verb (`lease.rs`). These are the `LEASE_HELD`/record-format contracts of the API — changing either is a design-authority change, not a refactor.

Donor code: reusable prior art lives in sibling repos (inventory with pinned SHAs in ARCHITECTURE.md §14). `/comprosody` and the iCloud snapshot dirs are explicitly non-donors — do not mine them.
