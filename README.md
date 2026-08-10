# CompOS

Writing and reading software with one document authority. It runs first as ordinary
processes on a Mac or Linux machine; the immutable Debian appliance (amd64 UEFI, then
Raspberry Pi) is a deployment target, not the product.

**Status: pre-implementation.** No code yet. The authoritative design is
[ARCHITECTURE.md](ARCHITECTURE.md) (v2.0, 2026-08-09) — a vetted and resequenced
revision of the original plan, with every external claim fact-checked, the donor-code
inventory verified against the repositories on disk, and a corrections changelog in
Appendix A.

Key decisions locked in v2.0:

- **All-purpose first:** the full product stack (composd + web shell) ships and is
  tested on macOS/Linux before any appliance image exists. Appliance work is Phases 6–7.
- **Rust** for `composd` and the service plane; TypeScript/React/CodeMirror 6 shell.
- **One API:** JSON-RPC 2.0 over UDS (services) and localhost WebSocket (shell), on
  every host profile; the command registry is the entire application surface.
- **Constitution enforced from day one** by in-process guards, file permissions, and a
  conformance test suite; systemd/EROFS hardening is additive at appliance time.

Next step: Phase 0 of the build plan (ARCHITECTURE.md §16) — workspace bootstrap,
donor relicensing, SHA pinning, and arm64 cross-compile CI.
