// The real cook CLI against the running Worker: a clean checkout at another path restores what the first one
// published, a second publish of the same input is not a conflict, and the other zone's token gets no hits.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startWorker } from "./start-worker.js";

const COOK = process.env.COOK_BIN ?? "cook";
const probe = spawnSync(COOK, ["--version"], { encoding: "utf8" });
const runnable = probe.status === 0;

// Each run appends to ran.log (outside the declared outputs) and prints RAN-MARKER, so a rerun is observable
// independently of what cook reports.
const COOKFILE = `recipe widget
    seal "src.txt"
    cook "build/widget.bin" {
        mkdir -p build
        echo ran >> ran.log
        echo RAN-MARKER
        sed 's/^/[widget] /' src.txt > $<out>
    }
`;

// Skipping is for a bare `pnpm test` without cook; a gate that names COOK_BIN must not pass on a skip.
test("cook shares builds through the Worker", { skip: runnable || process.env.COOK_BIN ? false : `cook not runnable (${COOK}): ${probe.error ?? probe.stderr}` }, async (t) => {
  assert.ok(runnable, `COOK_BIN is not runnable (${COOK}): ${probe.error ?? probe.stderr}`);
  const worker = await startWorker();
  const root = mkdtempSync(join(tmpdir(), "cook-e2e-"));
  t.after(async () => {
    await worker.stop();
    rmSync(root, { recursive: true, force: true });
  });

  // A checkout holds only source plus cloud.toml; HOME and the XDG dirs are private to it so no local store survives.
  function checkout(name, src) {
    const dir = join(root, name, "proj");
    mkdirSync(join(dir, ".cook"), { recursive: true });
    writeFileSync(join(dir, "Cookfile"), COOKFILE);
    writeFileSync(join(dir, "src.txt"), src);
    writeFileSync(
      join(dir, ".cook", "cloud.toml"),
      `[cloud]\nenabled = true\nproject = "e2e"\nendpoint = "${worker.url}"\nmax_artifact_mib = 32\ntimeout_secs = 10\nmax_retries = 1\n`,
    );
    return dir;
  }
  function cook(dir, token) {
    const home = join(dir, "..", "home");
    mkdirSync(home, { recursive: true });
    const env = {
      PATH: process.env.PATH,
      HOME: home,
      XDG_CACHE_HOME: join(home, "cache"),
      XDG_CONFIG_HOME: join(home, "config"),
      XDG_DATA_HOME: join(home, "data"),
      COOK_CLOUD_API_KEY: token,
    };
    // Async: the Worker runs in this process, so blocking here would starve it.
    return new Promise((resolve) => {
      const p = spawn(COOK, ["--output", "plain", "widget"], { cwd: dir, env });
      let out = "";
      p.stdout.on("data", (c) => (out += c));
      p.stderr.on("data", (c) => (out += c));
      p.on("close", (status) => resolve({ status, out }));
    });
  }
  const shown = (p) => `\n--- cook exit ${p.status} ---\n${p.out}`;
  const ran = (dir) => (existsSync(join(dir, "ran.log")) ? readFileSync(join(dir, "ran.log"), "utf8").split("\n").filter(Boolean).length : 0);
  const built = (dir) => readFileSync(join(dir, "build", "widget.bin"), "utf8");

  const A = checkout("a", "hello\n");
  const a = await cook(A, worker.trusted);
  assert.equal(a.status, 0, "A builds" + shown(a));
  assert.equal(ran(A), 1, "A's first build runs the recipe" + shown(a));
  assert.match(a.out, /RAN-MARKER/, shown(a));

  // B: different path, same source, empty local store.
  const B = checkout("b", "hello\n");
  const b = await cook(B, worker.trusted);
  assert.equal(b.status, 0, "B builds" + shown(b));
  assert.equal(ran(B), 0, "B restored from the shared store instead of rerunning" + shown(b));
  assert.doesNotMatch(b.out, /RAN-MARKER/, shown(b));
  assert.match(b.out, /1\/2 cached/, "cook reports the unit as cached" + shown(b));
  assert.equal(built(B), "[widget] hello\n", "B's output was restored" + shown(b));

  // Several clean checkouts of a new input miss together and each publishes the same key: no conflict.
  const peers = ["c1", "c2", "c3"].map((n) => checkout(n, "second\n"));
  const results = await Promise.all(peers.map((d) => cook(d, worker.trusted)));
  results.forEach((r, i) => {
    assert.equal(ran(peers[i]), 1, `${peers[i]} missed and ran the recipe itself` + shown(r));
    assert.equal(r.status, 0, `${peers[i]} builds` + shown(r));
    assert.doesNotMatch(r.out, /409|conflict/i, "publishing the same input is not a conflict" + shown(r));
  });
  // A later clean checkout of that input then hits what they published.
  const C = checkout("c4", "second\n");
  const c = await cook(C, worker.trusted);
  assert.equal(c.status, 0, shown(c));
  assert.equal(ran(C), 0, "later checkout restores the second input" + shown(c));

  // D: the other zone's token never sees the trusted zone's artifacts.
  const D = checkout("d", "hello\n");
  const d = await cook(D, worker.untrusted);
  assert.equal(d.status, 0, "D builds" + shown(d));
  assert.equal(ran(D), 1, "the untrusted zone gets no hits and reruns" + shown(d));
  assert.match(d.out, /RAN-MARKER/, shown(d));
});
