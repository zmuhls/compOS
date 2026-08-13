// Wire shapes of the composd API the shell consumes (ARCHITECTURE.md §6, §7).
// The registry is the API: everything below `hello` is a command result.

export interface CommandSpec {
  id: string;
  summary: string;
  effect: "read" | "propose" | "commit" | "system";
  network: string;
  default_keys: string[];
  input_schema: Record<string, unknown>;
}

export interface HelloResult {
  protocol: number;
  role_granted: string;
  capabilities: string[];
}

export interface DocumentEntry {
  doc: string;
  path: string;
  rev: string;
  object: string;
}

export interface DocumentContent extends DocumentEntry {
  content: string;
}

export interface SearchHit {
  doc: string;
  path: string;
  snippet: string;
}

export interface Hunk {
  start: number;
  del: number;
  ins: string;
}

export type ProposalStateName = "open" | "accepted" | "rejected" | "withdrawn";

export interface Proposal {
  proposal: string;
  doc: string | null;
  path: string;
  base: string | null;
  base_object: string | null;
  hunks: Hunk[];
  provenance: unknown;
  evidence: unknown;
  created_ms: number;
  state: ProposalStateName;
  stale: boolean;
  accepted_hunks?: number[];
  rev?: string;
  resolved_ms?: number;
}

export interface ProposalDetail extends Proposal {
  base_content: string;
}

export interface EventMsg {
  topic: string;
  seq: number;
  payload: Record<string, unknown>;
}

export interface RevisionCommittedPayload {
  doc: string;
  rev: string;
  object: string;
  path: string;
  origin: string;
}
