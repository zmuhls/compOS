# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CompOS is writing/reading software with one document authority (`composd`). **ARCHITECTURE.md is the design authority** — read the relevant section before changing anything structural; section numbers below refer to it. The build plan (§16) runs Phase 0 → 7; Phases 0 and 1 are complete (vault, objects, journal, save transaction, SQLite/FTS5 tier-2, external-edit watcher, command registry, compos-rpc, N/N-1 harness, round-trip harness, conformance suite, torture gates; both former `DECISION(user)` markers ratified 2026-08-11 — see the `RATIFIED` comments in `journal.rs` and `lease.rs`). Next: Phase 2 (web shell — Write + Search + basic Read).

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
pnpm install && pnpm -C shell typecheck && pnpm -C shell build   # web shell
cargo run -p composctl -- --vault /tmp/v init        # manual smoke
cargo run -p composd -- --vault /tmp/v               # run the daemon (uds + ws :7411)
```

`composctl` subcommands: `init save log cat search scan version` (+ hidden `_torture-child`). `composd` flags: `--vault --ws-port --socket --self-check`.

CI (`.github/workflows/ci.yml`) runs the rust matrix on macos-latest, ubuntu-latest, and ubuntu-24.04-arm (native arm64) plus the shell job. All must stay green — the arm runner exists so Pi problems surface years early.

Commits: short, lowercase, no sign-off.

## Architecture (the parts that span files)

Four crates: `compos-core` (pure sync library — all the semantics), `compos-rpc` (tokio UDS + WebSocket JSON-RPC boundary; role typing and capability caps live here, no document logic), `composd` (daemon: opens the vault, serves compos-rpc, runs the notify watcher), `composctl` (CLI client linking compos-core directly; also hosts the hidden `_torture-child` subcommand).

**The journal is truth.** `journal/` holds append-only JSONL records (canonical tier-1 state, §5.2); document heads are *derived* by replay in `DocIndex` — there is deliberately no pointer file and no second source of truth. `state/compos.db` (`derived.rs`) is a rebuildable FTS5 cache fed by the same replay: its **only** migration policy is destroy-and-rebuild on any `user_version` skew, and `tests/derived_rebuild.rs` proves delete → rebuild → identical results (rule 4). Anything that would create a second authority is a constitution violation.

**The save transaction** (`writer.rs`, §5.3) is six steps with exact fsync ordering: verify base+lease → object put → durable write-intent → atomic visible-file replace → journal append+fsync (**the acknowledgment point** — nothing is acked before this) → intent cleanup + derived-index feed (post-ack: a derived failure warns, never fails the save). `reconcile.rs` classifies every crash window (W1–W7 plus the external-edit edge); `tests/reconcile_windows.rs` hand-builds each window deterministically. If you touch the save path or reconciliation, the torture test (`crates/composctl/tests/torture.rs`) is the gate. Run it at 1000 iterations (and once with `TORTURE_DOCS=500`) before calling such a change done.

**Never clobber user bytes — enforced twice.** The save transaction itself has a guard: if the visible file matches neither the recorded head nor the incoming content, those bytes are committed as an `external` revision first and the save fails `StaleBase` (tests in `external_scan.rs`). `Vault::scan_external` sweeps the whole vault the same way; composd's notify watcher calls it on filesystem events. Hidden files (dot-prefixed components) are ignored by the scan; file deletion is report-only (no delete semantics in the Phase-1 model).

**Single-writer enforcement is by construction** (rule 1, §3): `VaultWriter` is the only mutation path, obtainable only from `Vault::open_write`, which holds an exclusive flock; a second writer gets `VaultBusy`. `VaultError` variants mirror the RPC wire errors 1:1 — `proto.rs::map_vault_error` is the single mapping; extend both sides together when adding variants.

**The registry is the API** (§7, `command.rs`): the RPC surface is six fixed methods, and everything else is a registry command with an effect class (`read < propose < commit < system`). Inputs are validated against the same JSON Schema `commands.describe` publishes. Role caps at the boundary (`session.rs`): shell/service ≤ commit, agent ≤ propose, maintenance ≤ system — an agent invoking a commit command gets `CAPABILITY_DENIED` before dispatch (rule 6; `tests/conformance.rs` is the constitution suite and must keep passing as-is).

**Frozen fixtures are contracts.** `compos-core/tests/fixtures/vault-format-N/` are golden vaults generated once by the build that introduced format N and never regenerated — `schema_compat.rs` forces every supported format to keep opening (tier-1 N/N-1 gate). `tests/fixtures/roundtrip/<codec-id>/` is the round-trip corpus (`codec.rs`; the `Codec` trait makes one-way codecs unrepresentable).

**Ratified contracts (2026-08-11, owner decision — do not revisit unilaterally):** the `JournalRecord` wire format is final for vault format 1 (short field names, `path` in-record, integer-ms `ts`, three-tier bump policy — details in `journal.rs`); leases are optional/advisory over base-matching, 60 s sliding TTL renewed by the holder's saves or explicit renew, auto-release on expiry, no steal verb (`lease.rs`). These are the `LEASE_HELD`/record-format contracts of the API — changing either is a design-authority change, not a refactor.

Donor code: reusable prior art lives in sibling repos (inventory with pinned SHAs in ARCHITECTURE.md §14). `/comprosody` and the iCloud snapshot dirs are explicitly non-donors — do not mine them.
