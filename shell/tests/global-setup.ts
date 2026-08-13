// Boots composd for the suite: init a scratch vault with composctl, start
// the daemon on a test port, wait for the startup token, and export the
// coordinates through the environment. The returned teardown kills the
// daemon and removes the vault.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WS_PORT = 7412;

function repoRoot(): string {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
}

function binary(name: string): string {
  const p = path.join(repoRoot(), "target", "debug", name);
  if (!existsSync(p)) {
    throw new Error(
      `${p} not found — build it first: cargo build -p composd -p composctl`,
    );
  }
  return p;
}

async function waitFor(
  what: string,
  probe: () => boolean | Promise<boolean>,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await probe()) {
      return;
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 100));
  }
}

function wsReachable(url: string): Promise<boolean> {
  return new Promise((resolve) => {
    const ws = new WebSocket(url);
    ws.onopen = () => {
      ws.close();
      resolve(true);
    };
    ws.onerror = () => resolve(false);
  });
}

export default async function globalSetup(): Promise<() => Promise<void>> {
  const composd = binary("composd");
  const composctl = binary("composctl");
  const vault = mkdtempSync(path.join(tmpdir(), "compos-e2e-"));

  const init = spawnSync(composctl, ["--vault", vault, "init"], {
    encoding: "utf8",
  });
  if (init.status !== 0) {
    throw new Error(`composctl init failed: ${init.stderr}${init.stdout}`);
  }

  const daemon = spawn(
    composd,
    [
      "--vault",
      vault,
      "--ws-port",
      String(WS_PORT),
      "--socket",
      path.join(vault, "e2e.sock"),
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  daemon.stderr.on("data", (d: Buffer) => {
    process.stderr.write(`[composd] ${d.toString()}`);
  });

  const tokenFile = path.join(vault, "state", "rpc-token");
  const wsUrl = `ws://127.0.0.1:${WS_PORT}/rpc`;
  try {
    await waitFor(
      "composd startup token",
      () => existsSync(tokenFile) && readFileSync(tokenFile, "utf8").length > 0,
      15_000,
    );
    await waitFor("composd WebSocket", () => wsReachable(wsUrl), 15_000);
  } catch (e) {
    daemon.kill("SIGKILL");
    throw e;
  }

  process.env["COMPOS_E2E_WS"] = wsUrl;
  process.env["COMPOS_E2E_TOKEN"] = readFileSync(tokenFile, "utf8").trim();
  process.env["COMPOS_E2E_VAULT"] = vault;

  return async () => {
    daemon.kill("SIGTERM");
    await new Promise((r) => setTimeout(r, 300));
    daemon.kill("SIGKILL");
    rmSync(vault, { recursive: true, force: true });
  };
}
