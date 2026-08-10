# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CompOS is writing/reading software with one document authority (`composd`). **ARCHITECTURE.md is the design authority** — read the relevant section before changing anything structural; section numbers below refer to it. The build plan (§16) runs Phase 0 → 7; Phases 0 and the Phase-1 vertical slice (vault, objects, journal, save transaction, composctl, torture test) are complete. Still ahead in Phase 1: compos-rpc (UDS + WebSocket JSON-RPC), SQLite/FTS5 derived indexes, the external-edit watcher, command registry, N/N-1 schema harness.

## Commands

```sh
cargo fmt --all --check                 # CI gate; run `cargo fmt --all` to fix
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # includes torture at 100 iterations
cargo test -p compos-core --test reconcile_windows   # one integration suite
cargo test -p compos-core save_and_read_back         # one test by name
TORTURE_ITERS=1000 cargo test -p composctl --test torture   # milestone gate
pnpm install && pnpm -C shell typecheck && pnpm -C shell build   # web shell
cargo run -p composctl -- --vault /tmp/v init        # manual smoke
```

CI (`.github/workflows/ci.yml`) runs the rust matrix on macos-latest, ubuntu-latest, and ubuntu-24.04-arm (native arm64) plus the shell job. All must stay green — the arm runner exists so Pi problems surface years early.

Commits: short, lowercase, no sign-off.

## Architecture (the parts that span files)

Three crates: `compos-core` (pure sync library — all the semantics), `composd` (daemon; currently a hello-world self-check, grows the RPC listeners), `composctl` (CLI client linking compos-core directly; also hosts the hidden `_torture-child` subcommand).

**The journal is truth.** `journal/` holds append-only JSONL records (canonical tier-1 state, §5.2); document heads are *derived* by replay in `DocIndex` — there is deliberately no pointer file and no second source of truth. SQLite, when it arrives, is a rebuildable cache fed by the same replay. Anything that would create a second authority is a constitution violation (rule 4).

**The save transaction** (`writer.rs`, §5.3) is six steps with exact fsync ordering: verify base+lease → object put → durable write-intent → atomic visible-file replace → journal append+fsync (**the acknowledgment point** — nothing is acked before this) → intent cleanup. `reconcile.rs` classifies every crash window (W1–W7 plus the external-edit edge, table in §5.3 and in the reconcile module docs); `tests/reconcile_windows.rs` hand-builds each window deterministically. If you touch the save path or reconciliation, the torture test (`crates/composctl/tests/torture.rs`) is the gate: it kill-−9s a child mid-save and asserts no acknowledged write is ever lost and no debris survives. Run it at 1000 iterations before calling such a change done.

**Single-writer enforcement is by construction** (rule 1, §3): `VaultWriter` is the only mutation path, obtainable only from `Vault::open_write`, which holds an exclusive flock; a second writer gets `VaultBusy`. `VaultError` variants deliberately mirror the future RPC wire errors (§6) — keep that 1:1 when adding variants.

**Never clobber user bytes**: external edits become `external` revisions (watcher, upcoming) or reconciliation warnings — silent overwrite is always a bug.

Two `DECISION(user):` markers await the owner's ratification and should not be resolved unilaterally: the `JournalRecord` wire format (`journal.rs`) and lease semantics (`lease.rs`).

Donor code: reusable prior art lives in sibling repos (inventory with pinned SHAs in ARCHITECTURE.md §14). `/comprosody` and the iCloud snapshot dirs are explicitly non-donors — do not mine them.
