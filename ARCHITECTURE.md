# CompOS Architecture

**Version 2.0 — 2026-08-09.** Supersedes the v1 "full operating-system theory" draft. Appendix A records every correction made against v1 and why. All external claims in this revision were fact-checked against primary sources on 2026-08-09; all donor-code claims were verified against the repositories on disk.

**Thesis.** CompOS is writing and reading software with one document authority. It runs first as ordinary processes on a Mac or Linux machine; the appliance is a deployment target, not the product. Everything that makes CompOS trustworthy — the single-writer vault, immutable revisions, proposal-gated AI, round-trip export law — is enforced in code and tests from day one, on every host. The immutable Debian image, A/B updates, and Raspberry Pi hardware arrive late, as packaging around a product that already works.

---

## 1. Product thesis and non-goals

CompOS opens directly into usable offline writing. One primary surface with five modes: **Write**, **Read**, **Capture**, **Review**, **Search**. Network services, cloud models, extensions, diagnostics, and repair activate only when their capabilities are needed.

The visual discipline is reMarkable-like: paper-toned monochrome surfaces, thin rules, generous margins, quiet typography, one restrained accent, progressive disclosure. Agent activity appears in the margin, the command palette, or Review mode. There is no permanent chatbot column — it would weaken the reading and writing focus.

Non-goals:

- Not a general-purpose computer. The appliance profiles run one application surface.
- Not a cloud service. Canonical data never requires a network to read, write, search, or recover.
- Not image-first. The OS image is Phase 6 of the build plan, not Phase 1 (see §16).
- Not a browser-authority app. React and CodeMirror render the shell; they never own the data. Browser storage holds nothing canonical.

## 2. Constitutional rules

These rules govern every subsystem. Each rule names its enforcement mechanism per host profile (profiles defined in §3); the principle throughout is that **enforcement is additive across profiles, the contract is constant** — dev machines enforce by construction and test, appliances add OS-level walls, and nothing about the rules themselves changes.

1. **`composd` is the sole writer of canonical documents, metadata, revisions, annotations, and project state.**
   *Dev:* a single `VaultWriter` type is the only code path that mutates the vault; an exclusive advisory lock on the vault makes a second writer fail loudly; the shell and all tools reach the vault only through the composd API. *Appliance:* additionally, per-service UIDs and `ProtectSystem=strict` with vault-only `ReadWritePaths=` make it unreachable by any other process.
2. **Writing, reading, saving, search, recovery, and local accessibility work without a model or network.** *All profiles:* CI runs the core suite with networking disabled.
3. **Canonical work remains ordinary UTF-8 Markdown, source files, media, bibliographies, and portable annotations.** *All profiles:* the round-trip fixture harness (§8) fails the build if any canonical byte becomes unreachable through plain files.
4. **SQLite indexes, FTS5, embeddings, thumbnails, OCR projections, and search indexes remain rebuildable.** This rule requires the three-tier state model in §5.2 — v1 silently violated it by implying revisions and job state lived only in SQLite. *All profiles:* a CI gate deletes the derived database and rebuilds it from canonical state; the rebuilt index must match.
5. **Agents receive bounded snapshots and return proposals, diagnostics, or artifacts against a specific base revision.** *All profiles:* snapshots are read-only copies with explicit scope; proposals carry `base_revision` and are rejected or routed to merge review when stale.
6. **Only `composd` can commit document changes. Only `repaird` can execute privileged system actions.** *Dev:* agent-role connections are capability-capped at effect ≤ `propose` inside the RPC boundary (§6); `repaird` actions run in dry-run mode with identical schemas. *Appliance:* the caps remain, and systemd sandboxing plus polkit-free fixed action enums make escalation structurally unavailable.
7. **Every Export has a registered importer and round-trip fixture.** *All profiles:* the codec registry rejects one-way codecs at registration time; CI enforces the round-trip law (§8).
8. **Ingested originals remain preserved even when conversion loses structure.** *All profiles:* ingest writes original bytes to the object store before any conversion runs.
9. **Production root filesystems never receive live code patches.** *Dev:* not applicable — dev machines are mutable by definition; the rule is exercised as dry-run update flows in Phase 5. *Appliance:* read-only EROFS/verity roots; `updated` writes only to the inactive slot.
10. **Accessibility, recovery, encryption, backup, and update rollback are base functions.** Restated honestly from v1: **in-app accessibility** (keyboard-complete operation, ARIA, contrast, zoom, reduced motion) is CompOS's job on every profile and is CI-gated from Phase 2. **Compositor-level screen-reader support is an ecosystem-gated risk**: Orca does not currently work on any wlroots compositor (no AT-SPI integration; the Newton project is the in-flight fix). See §15.4 and risk R2 — this rule cannot be honestly claimed as "base OS function, done" on the appliance today.

## 3. Host profiles

A `HostProfile` is resolved at composd startup. It selects paths, transports, process topology, and enforcement depth. It never changes the document model, the API contract, or command semantics.

| Profile | Topology | Vault location | Shell | Enforcement |
|---|---|---|---|---|
| `dev-mac` | Single composd process (+ sidecars as phases add them) | `~/Library/Application Support/compos/vault` | Ordinary browser at `http://127.0.0.1:7411` | Guards + permissions + tests |
| `dev-linux` | Same as dev-mac | `$XDG_DATA_HOME/compos/vault` | Ordinary browser | Guards + permissions + tests |
| `desktop` | Daemons under launchd / systemd-user | Same as dev | Browser or packaged webview | + service manager supervision |
| `appliance-amd64` | Full service plane under systemd | `/persistent/compos/` (LUKS2) | Kiosk browser under labwc | + systemd hardening, EROFS/verity roots, A/B updates |
| `appliance-pi` | Same as appliance-amd64 | Same | Same | Same, Pi boot chain (tryboot) |

Constitution enforcement on every profile rests on three layers:

1. **In-process guards.** All vault mutation flows through one `VaultWriter`; a vault `flock` rejects a second composd instance; RPC connections are role-typed and capability-capped at the boundary.
2. **File permissions.** Vault `0700`; agent snapshots are read-only copies in a separate directory; the shell auth token file is `0600`.
3. **The constitution conformance suite.** One test suite, run identically on macOS, Linux, and inside the appliance image in a VM. Examples: an agent connection invoking a commit-effect command receives `CAPABILITY_DENIED`; an out-of-band file edit becomes an `external` revision and never silently overwrites; a second composd instance refuses to start; deleting the SQLite database and rebuilding reproduces identical search results. This suite *is* the definition of profile equivalence.

Appliance profiles add the fourth layer — `DynamicUser`/per-service UIDs, `NoNewPrivileges`, `ProtectSystem=strict`, private tmp, address-family restrictions, seccomp, CPU/memory ceilings, socket activation, read-only verified roots — as documented in [`systemd.exec`](https://manpages.debian.org/trixie/systemd/systemd.exec.5.en.html). Nothing is removed when moving down to dev; nothing about the contract changes when moving up.

## 4. Runtime services and privileges

| Service | Responsibility | Canonical writes | Network | Language | Becomes a process in |
|---|---|---:|---:|---|---|
| CompOS shell | Write, Read, Capture, Review, Search | None | None | TypeScript/React | Phase 2 (browser tab) |
| `composd` | Vault, revisions, commands, codecs, projects, jobs, proposals | Yes | None | Rust | Phase 1 |
| `sttd` | whisper.cpp transcription | Results through `composd` | None | Rust (supervising a native worker) | Phase 4 |
| `modeld` | Embeddings, optional local classifiers | Derived data only | None | Rust | Phase 3 |
| `agentd` | ACP client host, MCP broker, context policy | Proposals only | Provider allowlist | Rust | Phase 3 |
| `extensiond` | Declarative and WASI extensions | Proposals and artifacts | Capability-dependent | Rust (wasmtime) | Phase 5 |
| `triaged` | Redacted system evidence | Diagnostic staging | None | Rust | Phase 5 |
| `harnessd` | Hermes, Claude, and Codex session coordination | Ephemeral sessions | Controlled | Rust | Phase 6+ |
| `repaird` | Fixed privileged repair operations | System state | None | Rust | Phase 5 (dry-run), Phase 6 (live) |
| `updated` | Verify, stage, promote, roll back releases | Inactive OS slot | Update endpoint | Rust | Phase 6 |

The staging rule that keeps nine daemons from being nine upfront projects: **daemon boundaries are contracts from day one; processes only exist when a phase needs them.** In Phase 1 exactly one process exists — composd. Embeddings, transcription, and agent hosting begin life as modules behind the same internal traits their daemons will later implement, and split out when their phase arrives. A daemon that never earns a phase never becomes a process.

Keep small, transactionally coupled functions inside `composd`. Separate heavy, networked, untrusted, or privileged execution into dedicated services.

## 5. `composd`: the canonical document authority

### 5.1 Persistent data layout

```text
<vault root>/                    (per-profile location, §3)
  vault/                  visible Markdown and source files
  objects/sha256/         immutable content objects: revisions, PDFs, EPUBs, audio, images
  journal/                append-only revision journal (canonical, §5.2)
  state/compos.db         derived graph, indexes, FTS5, job ledger (rebuildable)
  state/operations.db     bounded operational metrics (rebuildable)
  models/                 verified model packs
  skills/                 installed extension packages
  imports/staging/        quarantined imports
  exports/staging/        incomplete bundle construction
  triage/                 diagnostic and repair capsules
  backups/                consistent snapshots
  secrets/                encrypted service credentials
```

SHA-256 is portable object identity; BLAKE3 is fast local change detection. A stable document ID lives in reserved frontmatter, or in a portable sidecar where the source format cannot carry one.

### 5.2 The three-tier state model

v1 declared "SQLite remains rebuildable" while implicitly storing revisions and job state only in SQLite — a contradiction. The corrected model has three tiers:

1. **Canonical** — the visible files in `vault/`, the content-addressed objects in `objects/sha256/`, and the **append-only revision journal** in `journal/` (one JSONL segment per epoch; each record: document ID, revision ID, parent revision, object hash, origin ∈ {editor, external, proposal-accept, import}, timestamp). Losing anything here loses user data. This tier has its own **vault format version**, distinct from the SQLite schema version, with its own N/N-1 migration policy.
2. **Durable-derived** — `state/compos.db`: the document graph, FTS5, backlinks, citations, embeddings, thumbnails, job ledger. Deletable at any time; rebuilt from tier 1. CI proves this by doing it.
3. **Ephemeral** — document leases, RPC sessions, in-flight job state. Lost on restart by design; startup reconciliation (below) makes that safe.

**Proposals are canonical-adjacent durable state and live in composd** (journal + objects, indexed in SQLite) — not in agentd, not in the shell. Agents stay stateless; Review mode survives restarts.

### 5.3 The save transaction

A save transaction:

1. Verify the editor's base revision and document lease.
2. Write and flush a new immutable content object.
3. Register a write intent.
4. Atomically replace the visible Markdown file.
5. Append the revision record and advance the current pointer.
6. Clear the intent and refresh derived indexes.

Startup reconciles unfinished intents by object hash. External file changes (the user editing `vault/` with another tool) enter as explicit `external` revisions and never silently overwrite an active edit. The revision model is deliberately minimal — a linear chain per document plus content-addressed objects; no branching until a real feature demands it (risk R8).

Durability discipline is inherited from `dji-whisper-stream`'s tested pattern (append → flush → fsync before acknowledging; atomic rename for snapshots; documented shutdown ordering) — see §14.

### 5.4 Backups

Backups combine immutable objects with a SQLite snapshot taken by **`VACUUM INTO`** (or `sqlite3_rsync` when replicating to another machine); the C-level [Online Backup API](https://www.sqlite.org/backup.html) remains an option but is not the default. Never copy a live database and WAL pair while transactions may be in flight — mid-transaction copies can mix old and new pages ([How To Corrupt An SQLite Database File §1.2](https://www.sqlite.org/howtocorrupt.html)). A backup is green only after a scratch restore and test import pass.

## 6. The `composd` API contract

One protocol, two listeners, every profile.

- **Transport for services:** JSON-RPC 2.0, newline-delimited, over a Unix domain socket (`$XDG_RUNTIME_DIR/compos/composd.sock` on Linux, the app-support equivalent on macOS, `/run/compos/composd.sock` with socket activation on appliances). Peer identity via `SO_PEERCRED`/`getpeereid`.
- **Transport for the shell:** the same JSON-RPC payloads over WebSocket at `ws://127.0.0.1:7411/rpc`. This is **not a dev-only bridge** — the appliance kiosk browser cannot speak UDS either, so WebSocket is the frontend transport on every profile. Auth on all profiles: localhost bind plus a startup token written to a `0600` file and injected into the shell session.

The fixed protocol surface is six methods; **everything else is a registry command** (§7):

| Method | Purpose |
|---|---|
| `hello` | Version and role negotiation → `{protocol, role_granted, capabilities[]}` |
| `commands.list` | Registry enumeration; drives the command palette |
| `commands.describe` | Full schema for one command |
| `commands.invoke` | `{command, input, context: {doc_id?, base_revision?, lease_id?}, idempotency_key}` → result or `{job_id}` |
| `jobs.cancel` | Cancel a long-running invoke |
| `events.subscribe` | `{topics[]}`; server pushes `event` notifications `{topic, seq, payload}` |

Connections are role-typed at `hello`: `shell`, `service`, `agent`, `maintenance`. **An agent-role connection can never invoke an effect above `propose`; composd rejects commit-effect calls from agent connections at the boundary.** That is how "only composd commits" is enforced without systemd — the sandbox arrives later, the rule is enforced now.

Event topics include `revision.committed`, `doc.external_change`, `proposal.updated`, `proposal.stale`, `index.progress`, `job.progress`. The monotonic `seq` per topic lets a reconnecting shell detect gaps and resync. Typed errors in the application range: `STALE_BASE`, `LEASE_HELD`, `CAPABILITY_DENIED`, `VALIDATION_FAILED`, `VAULT_BUSY`.

Carrying to appliance changes exactly two things: role derivation (same-UID convention becomes per-service UIDs under systemd) and enforcement depth (§3 layer four). The contract is byte-identical.

## 7. One AI-native command surface

Every keyboard action, menu item, voice action, extension, and agent invocation addresses the same command registry. **The registry is the API**: commands are what `commands.invoke` invokes; there is no second, ad-hoc surface.

```ts
interface Command {
  id: string
  inputSchema: object
  outputSchema: object
  capabilities: string[]
  effect: "read" | "propose" | "commit" | "system"
  network: "never" | "provider-only" | "explicit"
  contextPolicy: string
  undoPolicy: string
  resourceClass: "interactive" | "background" | "maintenance"
  defaultKeys: string[]
}
```

Examples:

```text
document.navigate.heading      document.revise.selection
library.search.project         citation.insert
speech.transcribe.take         annotation.create
reading.explain.passage        proposal.accept.hunk
snapshot.open                  system.health.inspect
repair.propose
```

Profile gating: `system`-effect commands exist on **every** profile with identical schemas, and resolve to **dry-run** on dev and desktop profiles. The command surface is stable across profiles; only the blast radius changes.

An agent context package contains explicit project IDs, document revisions, selected ranges, permitted linked sources, sensitivity labels, and size limits. It never receives an implicit whole-vault context.

Every AI edit returns: base revision, proposed patch, affected stable IDs, evidence and source references, provider and model provenance, requested capabilities, verification results. `composd` rejects stale patches or routes them to merge review — and re-checks staleness **at accept time**, not only at proposal time (a pattern proven in `comprosody-reader`'s proposal state machine, §14).

## 8. Export, ingest, and publish symmetry

Three precise verbs:

- **Export** creates a portable representation with a tested importer.
- **Ingest** accepts an external format and preserves its original bytes.
- **Publish** renders a human-facing document. Every published format remains ingestible as a preserved source artifact.

The core CI law:

```text
logical_digest(resource) = logical_digest(import(export(resource)))
object hashes before export = object hashes after import
```

A codec registration must include both directions, identity rules, dependency closure, collision behavior, schema range, unknown-field preservation, fidelity classification, and golden fixtures. CompOS rejects one-way codecs. **The fixture harness exists from Phase 1** with the Markdown identity fixture — it is the first CI gate, not a late one.

| Domain | Outbound and return path | Guarantee |
|---|---|---|
| Complete project | `.compos ⇄ .compos` | Lossless graph and object identity |
| Markdown vault | Markdown directory / Textbundle ⇄ importer | Exact text and unknown frontmatter |
| Citations | CSL-JSON + original BibTeX/RIS ⇄ importer | Normalized fields plus original bytes |
| Annotations | W3C Web Annotation JSON-LD ⇄ importer | Bodies, selectors, targets, provenance |
| Audio/transcripts | WAV, FLAC, Opus, JSON, VTT, SRT ⇄ importer | Original audio and timing relationships |
| DOCX, ODT, HTML, LaTeX, EPUB | Publish + ingest via Pandoc/readers | Structural fidelity, original preserved |
| PDF | Publish + archival ingest | Publication preserved; no source-reconstruction claim |
| Profiles/keymaps | Versioned JSON ⇄ importer | Unknown settings retained |
| Skills | Signed source package ⇄ quarantine importer | Exact source, fresh capability review |
| Models | Thin manifest or licensed pack ⇄ verifier | Revision, hashes, license, runtime ABI |
| Diagnostics | `.compdiag ⇄ triage viewer` | Redacted evidence, version provenance |
| Repair plans | `.comprepair ⇄ repaird verifier` | Typed plan, signature, preconditions |
| Backups | Snapshot export ⇄ scratch restore | Green only after test import |

Annotations use the [W3C Web Annotation Data Model](https://www.w3.org/TR/annotation-model/): `TextQuoteSelector` (exact/prefix/suffix) and `TextPositionSelector` are first-class in the spec. **EPUB CFI is not a spec-defined selector type** — it enters via the generic `FragmentSelector` with `conformsTo` pointing at the IDPF CFI spec, and CompOS owns the CFI parse/validate logic (Readium's `readium/annotations` work is the prior art to watch). PDF anchors use the same quote-selector discipline; `readings-v2` contributes a proven text-anchored PDF locator format (§14).

A `.compos` bundle is ZIP64:

```text
manifest.json
records/*.jsonl
objects/sha256/<hash>
signatures/manifest.sig
```

Import follows `inspect → validate → plan → stage → commit → rebuild → report`. Reject path traversal, symlinks, device nodes, executable entries, decompression bombs, hash mismatches, duplicate paths, broken references, and unsupported required schemas. Preserve unknown future records in quarantine so CompOS can re-export them byte-identically.

## 9. The AI plane

Two interoperability protocols:

- **ACP** carries sessions, streamed messages, plans, diffs, tool activity, cancellation, and approval requests between `agentd` and agent providers. The current stable protocol is **version 1** (bidirectional JSON-RPC 2.0), but **v2 alphas are already published** (new session lifecycle, revised auth, new diff/patch format). `agentd` therefore treats the protocol version as **negotiated at session start**, records it in session metadata, and keeps an adapter layer between ACP wire formats and CompOS-native proposals — ACP diff-format churn stays at the adapter. [ACP repository](https://github.com/agentclientprotocol/agent-client-protocol).
- **MCP** exposes narrow CompOS capabilities (registry commands with agent-safe effects) to those providers.

```text
CompOS shell
    │ normalized agent events
    ▼
agentd — ACP client and policy owner
    ├── hermes acp                                  (stdio)
    ├── @agentclientprotocol/claude-agent-acp        (wraps the official Claude Agent SDK)
    └── @agentclientprotocol/codex-acp               (runs Codex App Server)
             │
             └── constrained CompOS MCP tools
```

Both adapters are maintained under the `agentclientprotocol` org (the old `@zed-industries` scopes are retired; the Zed codex repo is archived). Codex App Server provides threads, turns, approvals, cancellation, and streamed events over JSON-RPC. The [Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview) provides sessions, subagents, hooks, permissions, and MCP.

ACP supplies interoperability, **not** a security boundary. The boundary is composd's RPC capability cap (agent role ≤ `propose`) plus, on appliances, OS-level isolation.

**Prompt-injection posture, named explicitly** (v1 omitted it): document content, transcripts, and ingested files are **data, not instructions**, when they enter an agent context package. Context packages label their contents; system prompts instruct providers accordingly; and no agent output ever executes — it can only become a proposal that a human accepts hunk-by-hunk. The dictation-as-data prompt discipline in `dji-whisper-stream` is the local prior art.

### 9.1 Hermes placement

Hermes 0.20.0 supports Linux AArch64 (glibc + systemd + FHS), serves ACP over **stdio only** (its HTTP/SSE surface is a separate api-server feature), and provides MCP, sessions, subagents, and extensive tooling. Its own security policy states OS-level isolation supplies the real boundary.

Ship Hermes as an optional signed capsule with: no direct vault or root mount; no arbitrary terminal, `systemctl`, package manager, or bootloader access; no runtime pip/npm installation; no messaging gateways, cron, browser automation, plugins, or autonomous memory writes; a dedicated profile and private state directory; manual CompOS-owned approvals; only the first-party `compos-triage` MCP server.

Hermes does ship an ACP *client* shim today — but it is Copilot-shaped (spawns `copilot --acp` as an OpenAI-compatible backend), not a generalized ACP client that could orchestrate Claude and Codex peers (open feature requests: [#36057](https://github.com/NousResearch/hermes-agent/issues/36057), #5257, #16282). **`agentd` owns multi-provider orchestration**; that conclusion stands even if Hermes generalizes its client later.

### 9.2 Triage and repair flow

1. **Observe:** deterministic `composctl doctor` gathers redacted facts.
2. **Diagnose:** Hermes, Claude, or Codex may query allowlisted evidence.
3. **Propose:** the agent returns a typed repair plan or source patch.
4. **Apply:** `repaird` validates user approval and exact preconditions.
5. **Verify:** health checks run from fresh state; failure triggers rollback.

Allowlisted local repairs: restart a specific service, rebuild an index, quarantine an extension, prune a cache, select the prior slot, stage a signed update. `repaird` accepts action enums and typed arguments, never shell strings. On dev and desktop profiles the same actions run dry-run with identical schemas (§7).

Source repairs leave the appliance:

```text
device exports .compdiag
  → Mac or isolated builder imports it
  → Claude Code or Codex edits a disposable Git worktree
  → CI tests and signs an update
  → updated imports it into the inactive slot
  → health gate promotes or rolls back
```

### 9.3 Local model starter set (corrected)

- Whisper.cpp `base.en-q5_1` — ~57 MiB ([whisper.cpp models](https://huggingface.co/ggerganov/whisper.cpp)).
- Silero VAD — **~1.8 MB** (v1 claimed "under 1 MiB"; the ONNX/JIT artifact is roughly double that — still trivial, but budget tables must be honest).
- BGE Small EN v1.5 int8 ONNX — ~34 MB, **sourced from `onnx-community/bge-small-en-v1.5-ONNX` or Qdrant's mirror; upstream BAAI ships only fp32 (133 MB)**. Pin the correct repository in the model manifest.

Keep generative models out of the base Pi image. Ollama (local on capable hosts, Ollama Cloud, or a paired Mac) supplies generation and layout-heavy document parsing.

## 10. Extension architecture

Three extension levels:

1. **Declarative skills:** instructions, schemas, commands, tests, capability requests.
2. **WASI components** in `extensiond`, isolated with **three deliberate mechanisms, not one knob**:
   - capability sandboxing — WASI grants for specific files/directories only ([Wasmtime security](https://docs.wasmtime.dev/security.html));
   - CPU bounding — **epoch interruption** (chosen over fuel: ~10% overhead versus fuel's per-instruction cost; determinism of interrupt points is not a CompOS requirement) ([interrupting Wasm](https://docs.wasmtime.dev/examples-interrupting-wasm.html));
   - memory/table caps — the `ResourceLimiter` trait wired via `Store::limiter` ([ResourceLimiter](https://docs.wasmtime.dev/api/wasmtime/trait.ResourceLimiter.html)).
3. **Native providers:** signed STT, TTS, conversion, hardware, or model services delivered through OS updates.

Imported skills remain disabled until fresh capability approval. Extensions cannot replace top-level navigation, create arbitrary compositor windows, obtain credentials, or write canonical files. A hostile-extension containment test (infinite loop, memory balloon, filesystem escape attempt) is a Phase 5 gate.

Model packs live under `models/` with signed manifests: repository, immutable revision, SHA-256, SPDX license, byte size, runtime ABI, language, RAM limits, test vectors.

## 11. Maintenance plane

`triaged` produces `.compdiag` capsules — redacted evidence with version provenance; redaction is test-enforced (the donor pattern: a test asserting the payload matches no sensitive-field regex, from comprosody's `improvementMetrics.test.ts`). `repaird` executes fixed, typed, precondition-checked actions. `updated` verifies, stages, promotes, and rolls back releases against the inactive slot.

All three ship their schemas and dry-run behavior in Phase 5, on desktop profiles, so the appliance phase flips an enforcement bit rather than introducing new code paths.

## 12. The five frontend modes

- **Write:** CodeMirror 6, Markdown-aware navigation, history, Harper, Vale, Markdown Oxide, citation insertion, merge review. **Greenfield** — no donor CodeMirror code exists (all prior editors are Tiptap); budget accordingly.
- **Read:** PDF.js and EPUB.js, inherited from `readings-v2`: the pdf.js reader with render-scale caps tuned for weak GPUs, the text-anchored locator `pdfpage(N):text(start,end)` as the PDF analogue of a CFI, hard-won text/layout reconstruction (paragraph rebuild, running-head stripping, spacing repair), the EPUB builder/repair/validator pipeline, portable annotations, read-aloud with speech-unit→DOM alignment, linked passages.
- **Capture:** microphone recording with retained audio (donor: `audioStore.ts` take management), whisper.cpp transcription (new work; supervision pattern donated), real-time silence/duration segmentation (donor: `dji-whisper-stream` — RMS threshold, sustained-silence flush, max-segment force-flush), lexicon assistance with confirm/demote (donor: `lexicon.ts` phonetic mishearing detection), timestamped transcripts, prosody analysis (donor: `comprosody.ts` pure functions).
- **Review:** local diagnostics and AI proposals with per-hunk acceptance, stale-revision detection **re-checked at accept time**, evidence, undo (donor: `RefinementProposal` state machine with `streaming/ready/rejected/stale/failed` statuses; diff/variant UI components).
- **Search:** FTS5, backlinks, citations, annotations, optional BGE embeddings, source-opening commands.

The axe-core WCAG 2.2 suite from `comprosody-reader` (670 lines: reflow at 320px, forced-colors, reduced-motion, touch targets, focus restoration) becomes CI from Phase 2 — assertions and helpers inherited, selectors rewritten for the CompOS shell.

## 13. Implementation languages

- **`composd` and all service-plane daemons: Rust.** tokio (async runtime), rusqlite (SQLite), serde + jsonschema (command schemas), notify (external-edit watching), wasmtime (extensiond — Rust-native), whisper-rs or direct FFI (sttd). RPC is a small internal `compos-rpc` crate (UDS + WebSocket listeners, role typing, peer-cred auth) rather than a JSON-RPC framework — the dual-listener symmetry and capability caps fight framework assumptions.
- **Shell: TypeScript + React + CodeMirror 6**, talking only to the WebSocket API.
- **Python appears nowhere in the product.** Donor Python is reference material. One sanctioned exception: if the whisper.cpp spike fails its Phase 4 entry gate, a faster-whisper Python sidecar may run **behind the unchanged sttd contract** as a dev-profile stopgap, swapped out later without API change (risk R3).

## 14. Donor code inventory

The v1 plan's "Comprosody's bounded role" section pointed at the wrong repository. `/Users/milwright/Projects/dev/comprosody` is a stale, dirty snapshot (97 dirty files, HEAD 2026-07-02, five weeks behind its continuation); five of v1's eight reuse claims are false there and true in `comprosody-reader`. Corrected inventory, with SHAs pinned at audit time (2026-08-09):

| Donor | Pinned HEAD | Contributes | As |
|---|---|---|---|
| `comprosody-reader` | `51577aff5e51` (clean) | `src/lib/lexicon.ts` (phonetic mishearing detection, confirm/demote), `src/lib/audioStore.ts` (retained takes, blob/meta split), `RefinementProposal` state machine + accept-time stale recheck (`src/hooks/useRefinement.ts`, `RefinementSidecar.tsx`), 670-line axe-core WCAG 2.2 suite | Code + pattern |
| `comprosody-timed-release` | `a059371deade` (1 dirty file) | `server/lib/whisperWorker.ts` (engine-agnostic subprocess supervision: line-JSON protocol, UUID request map, timeouts, bounded auto-restart), transcription provider abstraction, DiffView/VariantCards/PassesBar review UI, `prompts.ts` faithful-edit policies, `PassageSelector {cfiRange, exact, prefix, suffix}` anchoring, Debian Dockerfile | Code + pattern + prompt text |
| `readings-v2` | `4441e85c913a` (clean, **already MIT**) | Everything PDF/EPUB (§12 Read), offline save-outbox coordinator (persisted outbox, retry lanes, offline/blocked/retrying states), dual-backend store interface + full DDL (including `library_book_versions` — versioned documents), TTS speech-unit→DOM alignment | Code |
| `dji-whisper-stream` | `a6a17d27960f` (clean) | Real-time segmentation policy (16 kHz mono, RMS silence threshold, 1.2 s flush, 8 s max segment), fsync durability layer (`DurableTextFile`/`AtomicTextFile`/`TranscriptJournal`) + shutdown ordering, stdlib streaming Ollama client, faithful-edit and citation-integrity system prompts, dictation-as-data injection hygiene | Pattern + prompt text (Python source is reference, not product code) |
| `Slide-Pi` | `f420cbf22d73` (clean) | better-sqlite3 + WAL + foreign-keys setup pattern, document→section→block schema shapes, pdf-parse/mammoth/turndown ingestion, optimistic-locking 409 pattern, nginx streaming proxy conf | Pattern |
| `milwrite` | `3d336345a778` (7 dirty files) | Voice guides as proposal-review system context; `check-source-fidelity.py` fabrication guard (rebuilt in Rust or invoked as fixture generator); writing-eval harness pattern | Prompt text + pattern |

**Explicit non-donors** (freeze read-only; never mine): `/Users/milwright/Projects/dev/comprosody` (stale + dirty), both `comprosody iCloud * 2026-07-28` snapshot directories, and `comprosody-timed-speech` (strict subset of timed-release; its only unique asset, the Anthropic SDK refinement adapter, is kept as a reference file).

**Negative findings, stated plainly** — these are new work, with no prior art in any local repo:

- No whisper.cpp anywhere (all transcription is Python faster-whisper). The *supervision* pattern transfers; the invocation does not.
- No CodeMirror anywhere (all editors are Tiptap). Write mode is greenfield.
- No revision chains, no content hashing, no proposal persistence anywhere. composd's revision model is a from-scratch build informed by three proven instincts: PassageSelector text anchoring, VoiceProfile's reject-unknown-versions migration discipline, and the EntryRevisionSnapshot + activation-counter staleness idea (right shape, wrong primitive — CompOS replaces string equality with revision IDs).
- No systemd units, kiosk configs, Wayland session code, or arm64 packaging anywhere.

**License action (Phase 0, not a blocker):** `readings-v2` already carries a root MIT license. The remaining donors are single-author; add MIT licenses before any donor code lands in the CompOS workspace.

## 15. The appliance track

### 15.1 Two build paths, not one declarative source

v1 claimed "two Linux images from one declarative source." The boot and update mechanisms genuinely differ, so this is **two build pipelines sharing component manifests** (package lists, service units, shell bundle, model packs):

- **`compos-uefi-amd64`** — built with [mkosi](https://github.com/systemd/mkosi): `Distribution=debian`, `Format=uki`, `SecureBoot=`, `Verity=` (repart emits root + Merkle hash + roothash-signature partitions), sysupdate integration. [ParticleOS](https://github.com/systemd/particleos) is the reference configuration — systemd's own image-based OS on exactly this stack (Fedora-first, so the Debian port is CompOS's work).
- **`compos-pi-arm64`** — built with [rpi-image-gen](https://github.com/raspberrypi/rpi-image-gen) and its `image-rota` A/B layer. Pi slot switching is driven by `autoboot.txt`/`tryboot` in the Pi firmware config partition — **not UEFI**; none of the amd64 boot chain transfers. The official `examples/webkiosk` (Cage + Chromium under systemd) is prior art for the kiosk unit, adapted to labwc.

mkosi builds arm64 images too (one config tree, `Architecture=` per invocation, qemu-user-static for cross-builds — two CI jobs, not one multi-arch build), but Pi A/B remains Pi-firmware-specific, so rpi-image-gen owns the Pi path. (rugix could unify both arches under one OTA engine, at the cost of abandoning systemd-sysupdate and the UKI/verity model — rejected while the systemd stack is the amd64 story.)

```text
BOOT          signed firmware, boot policy, slot metadata
ROOT_A        read-only CompOS release
ROOT_B        read-only CompOS release
RECOVERY      independent minimal recovery environment
PERSISTENT    LUKS2 user data, models, profiles, backups
```

Boot sequence: firmware → verified active slot → systemd → composd → Wayland session → full-screen CompOS shell. The shell opens without waiting for Wi-Fi, NTP, Ollama, indexing, or transcription workers.

### 15.2 Updates: sysupdate, repart, and the packaging traps

[`systemd-sysupdate`](https://manpages.debian.org/trixie/systemd-container/systemd-sysupdate.8.en.html) handles partition-based A/B transfers of UKIs and root images. Debian specifics that v1 missed:

- On Debian 13, `systemd-sysupdate` ships in the **`systemd-container`** binary package — easy to omit from a minimal image manifest.
- **`systemd-repart` is required, not optional**: sysupdate will not create partitions; repart materializes the B slot and the persistent partition on first boot.
- sysupdate's `--verify=` is GPG-over-SHA256SUMS on the *download* — orthogonal to Secure Boot. Both are needed.

### 15.3 Secure Boot key custody (the largest unresolved decision)

Debian does not ship pre-signed custom UKIs and will not sign CompOS's. **CompOS owns its signing keys.** The decided direction (executed at Phase 6 entry, because no earlier code depends on it):

- Dedicated appliance hardware: enroll a custom PK/KEK/db at provisioning; mkosi/ukify sign the UKI with the CompOS key; dm-verity roothash rides the kernel command line inside the signed UKI (the ParticleOS model).
- Shared or user-owned amd64 machines (desktop-adjacent installs): MOK enrollment via shim.
- v1 amd64 appliances may ship with a documented Secure-Boot-off fallback; the A/B + verity integrity story does not depend on Secure Boot, only the anti-evil-maid story does.

### 15.4 Compositor and accessibility, restated honestly

Use **labwc**, and not for v1's reason. Cage's limits are architectural — a single-app kiosk with no window cycling and immovable, force-centered dialogs — which is permanently hostile to Orca, IME candidate windows, and approval sheets; no accessibility matrix pass changes that. labwc is a stacking wlroots compositor with layer-shell, foreign-toplevel, session-lock, and documented IME integration, and it is Raspberry Pi OS's default compositor.

But the honest gate: **Orca does not work on any wlroots compositor today** — there is no AT-SPI integration on that stack, and the fix (the Newton project) is ecosystem work outside CompOS's control. So screen-reader support gates *both* candidates equally. Decision rule at Phase 6: if screen-reader operation is a hard launch requirement, run a Mutter-kiosk spike; otherwise ship labwc, state the limitation in release notes, and track Newton. Every in-app accessibility obligation (keyboard completeness, ARIA, contrast, zoom, reduced motion, switch-compatible command activation) is CI-enforced from Phase 2 regardless.

Ship AT-SPI, Speech Dispatcher, eSpeak NG, PipeWire, IMEs, RTL fonts, sticky keys, remappable keys, reduced motion, high contrast, 200–400% zoom. Recovery must remain keyboard and screen-reader operable — with the same ecosystem caveat until Newton lands.

Map physical typewriter scan codes once through udev; feed standard XKB keys or F13–F24 into the normal Wayland input path. System-wide key capture stays limited to power and guarded recovery commands.

### 15.5 Recovery

The recovery slot mounts persistent data read-only first. It can verify root slots, inspect storage, check SQLite integrity, rebuild derived indexes, export a rescue bundle, restore a snapshot, or reinstall an OS slot. Factory reset requires physical confirmation and offers rescue export first.

One local verification is a Phase 6 entry check: confirm Debian's stock kernel sets `CONFIG_EROFS_FS` (almost certainly `=m`, meaning the module must be in the initrd for a read-only EROFS root to boot).

## 16. Build plan — all-purpose first

The v1 sequence was image-first. Rejected: the dominant unknowns are the product layer, and constitutional rules 1–8 are implementable and testable as ordinary processes. Resequenced:

**Phase 0 — Bootstrap and mining.** Cargo workspace + pnpm shell skeleton. Relicense donors (MIT headers; readings-v2 done). Pin donor SHAs (§14 table). Freeze non-donors read-only. Stand up CI on macOS, Linux, **and arm64-linux cross-compilation from day one** — the one Pi risk that is nearly free to retire early. *Exit:* hello-world composd green on all three targets.

**Phase 1 — composd core (mac/linux).** Vault, SHA-256 content objects, linear revision chain + journal, the save transaction (§5.3), external-edit watcher → `external` revisions, SQLite + FTS5 as rebuildable tier-2, command registry with the four effect classes, `compos-rpc` (UDS + WS), a `composctl` CLI test client, and the round-trip fixture harness with the Markdown identity fixture. *Rules made real: 1, 3, 4, 6 (partially — no agents yet), 8 (basic ingest).* *Gates:* power-loss torture (kill −9 during save, ≥1,000 iterations, zero corruption or lost acknowledged writes), N/N-1 schema harness from the first migration, constitution conformance suite. *Exit:* a 500-document synthetic vault survives torture; a deleted-and-rebuilt index matches the live one.

**Phase 2 — Web shell: Write + Search + basic Read.** CodeMirror 6 (greenfield), command palette driven by `commands.list`, FTS5 search, Markdown reading. *Gates:* axe-core WCAG 2.2 suite in CI; keyboard-complete operation. *Exit:* the full write/save/search loop, with the shell touching the vault only through composd.

**Phase 3 — AI plane: proposals end-to-end.** `agentd` (ACP with negotiated version + adapter layer), proposal persistence in composd, Review mode with per-hunk acceptance and accept-time stale recheck, `modeld` (BGE int8 from onnx-community), MCP broker, prompt-injection hygiene. *Rules made real: 5, 6 fully.* *Gates:* stale-proposal rejection tests; agent-role commit-refusal tests. *Exit:* an agent proposes, the user accepts hunks, only composd commits, and proposals survive restart.

**Phase 4 — Capture and format breadth.** `sttd` with whisper.cpp — *entry criterion:* a one-week whisper.cpp spike passes (streaming transcription with word timestamps on mac + linux); otherwise the sanctioned faster-whisper sidecar engages behind the unchanged sttd contract. Silero VAD, dji-derived segmentation, retained audio, lexicon confirm/demote. PDF/EPUB/DOCX ingest via the codec registry, `.compos` export/import, Publish. *Rules: 2 (offline capture), 7, 8 fully.* *Gates:* per-codec round-trip fixtures; malicious-archive fuzzing begins. *Exit:* dictation → canonical document with retained audio; the round-trip law holds across the fixture corpus.

**Phase 5 — Desktop profile, extensions, maintenance plane (dry-run).** `extensiond` (WASI grants + epoch interruption + ResourceLimiter, hostile-extension containment tests), `triaged` with redaction tests, `repaird` dry-run with production schemas, launchd/systemd-user packaging for daily-driver use. *Rule 9 in dry-run form.* *Exit:* a hostile extension is contained; a `.compdiag` passes redaction verification.

**Phase 6 — appliance-amd64.** mkosi configuration from the ParticleOS reference, EROFS kernel check, the systemd hardening flip (§3 layer four), sysupdate + repart A/B, Secure Boot custody decision executed (§15.3), labwc kiosk session, `updated`/`repaird` live-privileged. *Rules 9, 10 fully (with §15.4's honest a11y caveat).* *Gates:* fresh flash + offline boot, A/B update and forced rollback, hardware power-loss during save, full-disk behavior with reserved recovery space.

**Phase 7 — appliance-pi.** rpi-image-gen/image-rota, tryboot A/B, shared component manifests, thermal and memory-pressure gates on target hardware. *If Phase 7 discovers product work, earlier phases leaked* — by design this phase is packaging, kiosk config, and hardware qualification only.

## 17. Release gates

The full gate list, unchanged in substance from v1, now with owning phases:

| Gate | Phase |
|---|---|
| Round-trip fixtures (`.compos`, Markdown, citations, annotations) | 1 (Markdown) → 4 (all codecs) |
| Power loss during document save | 1 (kill −9), 6 (hardware) |
| N and N-1 persistent-schema compatibility (SQLite **and** vault format) | 1 onward |
| Constitution conformance suite | 1 onward, in-image from 6 |
| Keyboard, screen reader (in-app), IME, microphone, audio tests | 2 onward |
| Provider outage and stale-proposal handling | 3 |
| Malicious archive fuzzing | 4 onward |
| Backup test import | 4 onward |
| Extension containment (runaway CPU/memory, fs escape) | 5 |
| Fresh flash and offline boot | 6, 7 |
| A-to-B update and forced rollback | 6, 7 |
| Full-disk behavior with reserved recovery space | 6, 7 |
| Thermal and memory-pressure tests on target Pi hardware | 7 |
| SBOM, dependency, model-hash, license verification | 0 onward, per release |

## 18. Risk register

| # | Risk | Mitigation (decided) |
|---|---|---|
| R1 | **Secure Boot key custody** — Debian won't pre-sign custom UKIs | Own the keys: custom db enrollment on dedicated hardware, MOK on shared machines, documented SB-off fallback for v1 amd64. Decide at Phase 6 entry; zero earlier code depends on it. |
| R2 | **wlroots screen-reader gap** — Orca works on no wlroots compositor | Split the concern: in-app a11y CI-gated from Phase 2 unconditionally; compositor a11y tracked against Newton; Mutter-kiosk spike if Orca becomes a hard requirement at Phase 6. |
| R3 | **whisper.cpp novelty** — zero prior art in donor repos | One-week spike as Phase 4 entry criterion; supervision pattern transfers from `whisperWorker.ts`; fallback is the faster-whisper sidecar behind the unchanged sttd contract. |
| R4 | **ACP v2 churn** — session lifecycle and diff format in flux | Negotiate protocol version per session; CompOS-native proposal format; adapter layer absorbs wire changes. |
| R5 | **Nine-daemon scope breadth** | Staging rule (§4): contracts from day one, processes only when a phase requires them; Phase 1 is one process. |
| R6 | **License formality on donor code** | Phase 0 action item; readings-v2 already MIT; remaining repos single-author. Done before any donor code lands. |
| R7 | **Dirty/duplicate donor repos** | Non-donor freeze list (§14); mine only clean trees; SHAs pinned in the inventory. Commit the 1 dirty file in `comprosody-timed-release` and 7 in `milwrite` before mining them. |
| R8 | **Revision-model novelty** — no revision chains or hashing exist anywhere locally | Keep the model minimal (linear chain, content-addressed objects, no branching); crash-torture from day one; borrow the three proven donor instincts as *tests*, not as code. |

---

## Appendix A — Corrections changelog (v1 → v2)

| v1 claim | v2 correction | Source |
|---|---|---|
| "Build two Linux images from one declarative source" | Two build pipelines (mkosi amd64-UEFI; rpi-image-gen Pi-tryboot) sharing component manifests only — Pi A/B is firmware-specific, not UEFI | rpi-image-gen docs; mkosi manpage |
| systemd-sysupdate presented as sufficient for A/B | repart required to create partitions; sysupdate ships in `systemd-container` on Debian; `--verify` is GPG-on-download, orthogonal to Secure Boot | sysupdate.d(5), systemd-sysupdate(8), packages.debian.org |
| Signed UKIs implied to ride Debian's signing chain | Debian does not pre-sign custom UKIs; CompOS owns its Secure Boot keys (custom db or MOK); ParticleOS is the reference model | wiki.debian.org/SecureBoot; systemd/particleos |
| "Use labwc initially because Orca … may require auxiliary Wayland clients. A Cage profile can follow once the accessibility matrix passes" | labwc chosen for architectural reasons (Cage: single-app, no cycling, immovable dialogs — permanent); Orca works on **no** wlroots compositor (no AT-SPI; Newton in flight); a11y gates both candidates; Mutter spike is the fallback | cage-kiosk/cage; labwc integration docs; wayland-accessibility-notes |
| "Backups use … SQLite's Online Backup API. Never sync or copy a live database and WAL pair" | `VACUUM INTO` / `sqlite3_rsync` preferred for an appliance; Backup API remains valid; live db+WAL copy is unsafe *mid-transaction* (the operative case), not categorically | sqlite.org/howtocorrupt.html §1.2 |
| Wasmtime cited via security.html for "fuel, memory, time" limits | Three separate mechanisms: WASI capability grants; epoch interruption (chosen) or fuel for CPU; ResourceLimiter for memory | wasmtime docs (3 pages, §10) |
| Silero VAD "under 1 MiB" | ~1.8 MB | snakers4/silero-vad |
| BGE Small EN v1.5 "q8 ONNX ~34 MB" (source unstated) | Correct size, but int8 build comes from onnx-community/Qdrant; upstream BAAI is fp32-only (133 MB) | HF onnx-community repo |
| ACP "stable protocol is currently version 1" | Still true, but v2 alphas published (session lifecycle, diff format) — version is negotiated per session, adapter layer required | agentclientprotocol repo releases |
| Hermes "does not provide the general ACP client role" (issue #36057) | Directionally right, overstated: a Copilot-shaped ACP client shim exists; the *generalized* client is missing (#36057, #5257, #16282); Hermes ACP is stdio-only, HTTP/SSE is a separate api-server | hermes-agent repo, issues |
| "Repurpose these components [from Comprosody] after license resolution" (pointing at `/comprosody`) | `/comprosody` is a stale dirty snapshot; five of eight reuse claims are true only in `comprosody-reader`; six-donor inventory in §14; readings-v2 already MIT; no whisper.cpp/CodeMirror/revision-chain prior art exists anywhere | on-disk audit 2026-08-09 |
| "The present repository lacks a license, so copied source cannot enter CompOS until the owner supplies one" | All donor repos are single-author (the CompOS author); relicensing is a Phase 0 action item, not an external blocker; readings-v2 already carries MIT | on-disk audit |
| Build sequence step 1: "Immutable image, recovery slot, composd, and the visible Markdown vault" | Image work moved to Phases 6–7; Phase 1 is composd core as ordinary processes on mac/linux; constitution enforced by guards + tests until systemd hardening is additive at Phase 6 | user decision, 2026-08-09 |
| (absent) | Three-tier state model resolving the rule-4 contradiction; composd API contract; host profiles; implementation languages (Rust); prompt-injection posture; risk register | v2 additions |

## Appendix B — Glossary

- **`.compos`** — ZIP64 project bundle: manifest, JSONL records, content objects, signatures. Round-trip-guaranteed.
- **`.compdiag` / `.comprepair`** — redacted diagnostic capsule / typed, signed repair plan.
- **Canonical / durable-derived / ephemeral** — the three state tiers (§5.2).
- **Effect classes** — `read | propose | commit | system`; every command declares one; roles are capped by maximum effect.
- **Host profile** — the startup-resolved deployment shape (§3); changes enforcement depth, never contracts.
- **`logical_digest`** — the content-identity function over a resource's canonical form, used by the round-trip law.
- **Proposal** — a revision-anchored, evidence-carrying patch from an agent or diagnostic; inert until a human commits hunks through composd.
- **Vault format version** — the version of the on-disk canonical layout (journal + objects), migrated independently of the SQLite schema.
- **Write intent** — the crash-recovery marker registered before the visible-file replacement in a save transaction.
