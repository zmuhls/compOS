// The CompOS shell (ARCHITECTURE.md §12, §16 Phase 2): Write, Read, Search,
// and Review over the composd WebSocket API — the shell holds no canonical
// state and touches the vault only through commands. Keyboard-complete by
// construction: every operation is reachable through the palette (Mod-P),
// and shortcuts are accelerators on top.

import { useCallback, useEffect, useRef, useState } from "react";
import { Connect } from "./Connect";
import { RpcClient, RpcError } from "./rpc";
import { Palette } from "./ui/Palette";
import type { PaletteItem } from "./ui/Palette";
import { PromptDialog } from "./ui/PromptDialog";
import { RawInvoke } from "./ui/RawInvoke";
import { HelpDialog } from "./ui/HelpDialog";
import { WriteMode } from "./modes/WriteMode";
import { ReadMode } from "./modes/ReadMode";
import { SearchMode } from "./modes/SearchMode";
import { ReviewMode } from "./modes/ReviewMode";
import type {
  CommandSpec,
  DocumentContent,
  DocumentEntry,
  EventMsg,
  HelloResult,
  Proposal,
  RevisionCommittedPayload,
} from "./types";

export interface OpenDoc {
  /** Editor identity: changes when a different text should replace the view. */
  key: string;
  path: string;
  baseRev: string | null;
  text: string;
  dirty: boolean;
  /** Head revision the document moved to while this buffer was dirty. */
  conflict: string | null;
}

type Mode = "write" | "read" | "search" | "review";

interface Session {
  client: RpcClient;
  hello: HelloResult;
  url: string;
}

type DialogState =
  | { kind: "none" }
  | { kind: "palette"; title: string; items: PaletteItem[] }
  | { kind: "newdoc" }
  | { kind: "invoke"; spec: CommandSpec }
  | { kind: "help" };

const MODES: { id: Mode; label: string }[] = [
  { id: "write", label: "Write" },
  { id: "read", label: "Read" },
  { id: "search", label: "Search" },
  { id: "review", label: "Review" },
];

const DEFAULT_WS = "ws://127.0.0.1:7411/rpc";

function storedOr(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

export function App() {
  const params = useRef(new URLSearchParams(window.location.search));
  const [session, setSession] = useState<Session | null>(null);
  const [connectNotice, setConnectNotice] = useState<string | null>(null);
  const [commands, setCommands] = useState<CommandSpec[]>([]);
  const [docs, setDocs] = useState<DocumentEntry[]>([]);
  const [current, setCurrent] = useState<OpenDoc | null>(null);
  const [mode, setMode] = useState<Mode>("write");
  const [status, setStatus] = useState<{ kind: "info" | "error"; text: string } | null>(null);
  const [dialog, setDialog] = useState<DialogState>({ kind: "none" });
  const [proposalsVersion, setProposalsVersion] = useState(0);
  const [openProposals, setOpenProposals] = useState(0);
  const [editorFocusTick, setEditorFocusTick] = useState(0);

  const currentRef = useRef(current);
  currentRef.current = current;
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const dialogRef = useRef(dialog);
  dialogRef.current = dialog;

  const say = useCallback((kind: "info" | "error", text: string) => {
    setStatus({ kind, text });
  }, []);

  const refreshDocs = useCallback(async (client: RpcClient) => {
    const res = await client.invoke<{ documents: DocumentEntry[] }>("document.list", {});
    setDocs(res.documents);
  }, []);

  const refreshBadge = useCallback(async (client: RpcClient) => {
    const res = await client.invoke<{ proposals: Proposal[] }>("proposal.list", {
      state: "open",
    });
    setOpenProposals(res.proposals.length);
  }, []);

  const openDoc = useCallback(
    async (path: string, opts?: { switchTo?: Mode; focus?: boolean }) => {
      const s = sessionRef.current;
      if (s === null) {
        return;
      }
      try {
        const d = await s.client.invoke<DocumentContent>("document.read", { path });
        setCurrent({
          key: `${d.path}@${d.rev}`,
          path: d.path,
          baseRev: d.rev,
          text: d.content,
          dirty: false,
          conflict: null,
        });
        if (opts?.switchTo !== undefined) {
          setMode(opts.switchTo);
        }
        if (opts?.focus === true) {
          setEditorFocusTick((t) => t + 1);
        }
      } catch (e) {
        say("error", e instanceof RpcError ? e.message : String(e));
      }
    },
    [say],
  );

  // A commit event for the open document, applied once it is safe: reload a
  // clean buffer, flag a dirty one as conflicted.
  const applyCommitEvent = useCallback(
    (p: RevisionCommittedPayload) => {
      const cur = currentRef.current;
      if (cur === null || p.path !== cur.path || p.rev === cur.baseRev) {
        return;
      }
      if (cur.dirty) {
        setCurrent((prev) =>
          prev !== null && prev.path === cur.path
            ? { ...prev, conflict: p.rev }
            : prev,
        );
        say("error", `${cur.path} moved to a new revision while you edit (${p.origin}).`);
      } else {
        void openDoc(cur.path);
        // Accepts already announce themselves through Review mode; a second
        // status line would bury that confirmation.
        if (p.origin !== "proposal-accept") {
          say("info", `${cur.path} refreshed from composd (${p.origin}).`);
        }
      }
    },
    [openDoc, say],
  );
  const deferredCommitsRef = useRef<RevisionCommittedPayload[]>([]);

  const savingRef = useRef(false);
  const save = useCallback(async () => {
    const s = sessionRef.current;
    const cur = currentRef.current;
    if (s === null || cur === null || savingRef.current) {
      return;
    }
    savingRef.current = true;
    try {
      const r = await s.client.invoke<{ rev: string }>("document.save", {
        path: cur.path,
        content: cur.text,
        base: cur.baseRev,
      });
      setCurrent((prev) =>
        prev !== null && prev.path === cur.path
          ? {
              ...prev,
              baseRev: r.rev,
              dirty: prev.text !== cur.text,
              conflict: null,
            }
          : prev,
      );
      say("info", `Saved ${cur.path} (${r.rev.slice(0, 10)}…)`);
      await refreshDocs(s.client);
    } catch (e) {
      if (e instanceof RpcError && e.type === "STALE_BASE") {
        const head = await s.client
          .invoke<DocumentContent>("document.read", { path: cur.path })
          .catch(() => null);
        setCurrent((prev) =>
          prev !== null && prev.path === cur.path
            ? { ...prev, conflict: head?.rev ?? "unknown" }
            : prev,
        );
        say(
          "error",
          "Stale base: the document changed underneath this buffer. Nothing was overwritten.",
        );
      } else {
        say("error", e instanceof RpcError ? `${e.type}: ${e.message}` : String(e));
      }
    } finally {
      savingRef.current = false;
      const deferred = deferredCommitsRef.current;
      deferredCommitsRef.current = [];
      // Events that arrived mid-save: our own commit is now recognized by
      // its baseRev and ignored; anyone else's still gets the full
      // conflict/refresh treatment.
      for (const p of deferred) {
        applyCommitEvent(p);
      }
    }
  }, [say, refreshDocs, applyCommitEvent]);

  const handleEvent = useCallback(
    (ev: EventMsg) => {
      const s = sessionRef.current;
      if (s === null) {
        return;
      }
      if (ev.topic === "revision.committed" || ev.topic === "doc.external_change") {
        void refreshDocs(s.client);
      }
      if (ev.topic === "revision.committed") {
        const p = ev.payload as unknown as RevisionCommittedPayload;
        const cur = currentRef.current;
        if (cur !== null && p.path === cur.path) {
          if (savingRef.current) {
            // Our own save's event can outrun the save response; judging it
            // now would flag a false conflict. Reconcile after the save.
            deferredCommitsRef.current.push(p);
          } else {
            applyCommitEvent(p);
          }
        }
      }
      if (ev.topic === "proposal.updated" || ev.topic === "proposal.stale") {
        setProposalsVersion((v) => v + 1);
        void refreshBadge(s.client);
      }
    },
    [applyCommitEvent, refreshBadge, refreshDocs],
  );

  const attachSession = useCallback(
    async (client: RpcClient, hello: HelloResult, url: string, token: string) => {
      try {
        localStorage.setItem("compos.ws", url);
        localStorage.setItem("compos.token", token);
      } catch {
        // Private browsing: connection still works, it just isn't remembered.
      }
      setSession({ client, hello, url });
      setConnectNotice(null);
      client.onEvent(handleEvent);
      client.onGap(() => {
        const s = sessionRef.current;
        if (s !== null) {
          void refreshDocs(s.client);
          void refreshBadge(s.client);
          setProposalsVersion((v) => v + 1);
        }
      });
      client.onClose(() => {
        setSession(null);
        setConnectNotice("Connection to composd closed. Is the daemon still running?");
      });
      const listed = await client.request<{ commands: CommandSpec[] }>("commands.list", {});
      setCommands(listed.commands);
      await client.subscribe([
        "revision.committed",
        "doc.external_change",
        "proposal.updated",
        "proposal.stale",
      ]);
      await refreshDocs(client);
      await refreshBadge(client);
      say("info", `Connected as ${hello.role_granted} (protocol ${hello.protocol}).`);
    },
    [handleEvent, refreshBadge, refreshDocs, say],
  );

  // ?token= (and optional ?ws=) supports launch-from-terminal and the e2e
  // harness; the token is scrubbed from the address bar after the attempt.
  useEffect(() => {
    const token = params.current.get("token");
    if (token === null || token === "") {
      return;
    }
    const url = params.current.get("ws") ?? storedOr("compos.ws", DEFAULT_WS);
    window.history.replaceState(null, "", window.location.pathname);
    RpcClient.connect(url, token).then(
      ({ client, hello }) => void attachSession(client, hello, url, token),
      (e: Error) => setConnectNotice(`Auto-connect failed: ${e.message}`),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const label = MODES.find((m) => m.id === mode)?.label ?? "Shell";
    document.title = `CompOS — ${label}`;
  }, [mode]);

  // Global accelerators. Dialogs own their keyboard while open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (sessionRef.current === null) {
        return;
      }
      const mod = e.metaKey || e.ctrlKey;
      if (dialogRef.current.kind !== "none") {
        return;
      }
      if (mod && !e.shiftKey && (e.key === "p" || e.key === "P")) {
        e.preventDefault();
        openPaletteRef.current();
      } else if (mod && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setMode("search");
      } else if (mod && (e.key === "s" || e.key === "S")) {
        // Inside the editor the CodeMirror keymap already handled Mod-S;
        // acting here too would fire a second, stale-base save.
        const target = e.target instanceof Element ? e.target : null;
        if (target?.closest(".cm-editor") === null) {
          e.preventDefault();
          void save();
        }
      } else if (e.altKey && !mod) {
        const codes: Record<string, Mode> = {
          Digit1: "write",
          Digit2: "read",
          Digit3: "search",
          Digit4: "review",
        };
        const m = codes[e.code];
        if (m !== undefined) {
          e.preventDefault();
          setMode(m);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [save]);

  const openPalette = useCallback(() => {
    const s = sessionRef.current;
    if (s === null) {
      return;
    }
    const cur = currentRef.current;
    const items: PaletteItem[] = [];
    for (const m of MODES) {
      items.push({
        id: `mode:${m.id}`,
        label: `Go to ${m.label}`,
        keys: [`Alt-${MODES.indexOf(m) + 1}`],
        run: () => setMode(m.id),
      });
    }
    items.push({
      id: "doc:new",
      label: "New document…",
      run: () => setDialog({ kind: "newdoc" }),
    });
    items.push({
      id: "doc:open",
      label: "Open document…",
      run: () => {
        const docItems = docs.map<PaletteItem>((d) => ({
          id: `open:${d.doc}`,
          label: d.path,
          hint: d.rev.slice(0, 12),
          run: () =>
            void openDoc(d.path, {
              ...(mode === "search" || mode === "review"
                ? { switchTo: "write" as const }
                : {}),
              focus: true,
            }),
        }));
        setDialog({ kind: "palette", title: "Open document", items: docItems });
      },
    });
    if (cur !== null) {
      items.push({
        id: "doc:save",
        label: `Save ${cur.path}`,
        keys: ["Mod-S"],
        run: () => void save(),
      });
      items.push({
        id: "doc:reload",
        label: `Reload ${cur.path} from head`,
        run: () => void openDoc(cur.path, { focus: true }),
      });
    }
    items.push({
      id: "help",
      label: "Keyboard help",
      run: () => setDialog({ kind: "help" }),
    });
    items.push({
      id: "disconnect",
      label: "Disconnect",
      run: () => {
        sessionRef.current?.client.close();
      },
    });
    // The registry drives the rest (§7: the registry is the API). Commands
    // with first-class UI route there; everything else opens raw invoke.
    for (const spec of commands) {
      items.push({
        id: `cmd:${spec.id}`,
        label: spec.id,
        hint: spec.summary,
        keys: spec.default_keys,
        run: () => {
          if (spec.id === "document.save") {
            void save();
          } else if (spec.id === "search.query") {
            setMode("search");
          } else if (spec.id.startsWith("proposal.")) {
            setMode("review");
          } else if (spec.id === "document.read" || spec.id === "document.list") {
            setMode("write");
          } else {
            setDialog({ kind: "invoke", spec });
          }
        },
      });
    }
    setDialog({ kind: "palette", title: "Command palette", items });
  }, [commands, docs, mode, openDoc, save]);
  const openPaletteRef = useRef(openPalette);
  openPaletteRef.current = openPalette;

  if (session === null) {
    return (
      <Connect
        initialUrl={storedOr("compos.ws", DEFAULT_WS)}
        initialToken={storedOr("compos.token", "")}
        notice={connectNotice}
        onConnected={(client, hello, url, token) =>
          void attachSession(client, hello, url, token)
        }
      />
    );
  }

  return (
    <div className="app">
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <header className="app-header">
        <p className="brand">CompOS</p>
        <nav aria-label="Modes">
          {MODES.map((m) => (
            <button
              key={m.id}
              type="button"
              aria-current={mode === m.id ? "page" : undefined}
              onClick={() => setMode(m.id)}
            >
              {m.label}
              {m.id === "review" && openProposals > 0 ? ` (${openProposals})` : ""}
            </button>
          ))}
        </nav>
        <div className="conn">
          <button type="button" onClick={openPalette}>
            Palette
          </button>
        </div>
      </header>
      <main id="main" className="app-main">
        {mode === "write" && (
          <WriteMode
            docs={docs}
            current={current}
            focusTick={editorFocusTick}
            onOpen={(p) => void openDoc(p, { focus: true })}
            onNew={() => setDialog({ kind: "newdoc" })}
            onChange={(text) =>
              setCurrent((prev) =>
                prev !== null ? { ...prev, text, dirty: true } : prev,
              )
            }
            onSave={() => void save()}
            onReloadHead={() => {
              const cur = currentRef.current;
              if (cur !== null) {
                void openDoc(cur.path, { focus: true });
              }
            }}
          />
        )}
        {mode === "read" && <ReadMode current={current} />}
        {mode === "search" && (
          <SearchMode
            client={session.client}
            onOpen={(p) => void openDoc(p, { switchTo: "write", focus: true })}
          />
        )}
        {mode === "review" && (
          <ReviewMode
            client={session.client}
            version={proposalsVersion}
            onStatus={say}
          />
        )}
      </main>
      <footer className="statusbar">
        <p
          role="status"
          aria-live="polite"
          className={status?.kind === "error" ? "status-text error" : "status-text"}
        >
          {status?.text ?? "Ready."}
        </p>
        <p className="conn-meta">
          {docs.length} document{docs.length === 1 ? "" : "s"} ·{" "}
          {session.hello.role_granted} · {session.url}
        </p>
      </footer>
      {dialog.kind === "palette" && (
        <Palette
          title={dialog.title}
          items={dialog.items}
          onClose={() => setDialog({ kind: "none" })}
        />
      )}
      {dialog.kind === "newdoc" && (
        <PromptDialog
          title="New document"
          label="Vault path (for example notes/idea.md)"
          submitLabel="Create"
          onSubmit={(path) => {
            setCurrent({
              key: `new:${path}`,
              path,
              baseRev: null,
              text: "",
              dirty: true,
              conflict: null,
            });
            setMode("write");
            setEditorFocusTick((t) => t + 1);
          }}
          onClose={() => setDialog({ kind: "none" })}
        />
      )}
      {dialog.kind === "invoke" && (
        <RawInvoke
          client={session.client}
          spec={dialog.spec}
          onClose={() => setDialog({ kind: "none" })}
        />
      )}
      {dialog.kind === "help" && (
        <HelpDialog onClose={() => setDialog({ kind: "none" })} />
      )}
    </div>
  );
}
