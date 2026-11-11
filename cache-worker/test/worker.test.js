// node --test test/worker.test.js: real HTTP against the Worker on a local R2.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { after, before, test } from "node:test";
import { startWorker } from "./start-worker.js";

let w;
before(async () => { w = await startWorker(); });
after(async () => { await w.stop(); });

const ZERO = "0".repeat(64);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const newKey = () => randomBytes(32).toString("hex");
const call = (path, { token = w.trusted, ...init } = {}) =>
  fetch(w.url + path, { ...init, headers: { ...(token && { Authorization: `Bearer ${token}` }), ...init.headers } });
const meta = { "X-Cook-Content-Hash": ZERO, "X-Cook-Size-Bytes": "5", "X-Cook-Schema-Version": "3", "X-Cook-Recipe-Namespace": "ns",
  "X-Cook-Output-Index": "1", "X-Cook-Output-Path": "out/a.o", "X-Cook-Mode": "493", "X-Cook-Kind": "", "X-Cook-Symlink-Target": "" };
const put = (key, body, opts = {}) => call(`/v1/artifacts/${key}`, { method: "PUT", body, headers: meta, ...opts });
const query = (keys, opts = {}) => call("/v1/artifacts/batch_query", { method: "POST", body: JSON.stringify({ keys }), ...opts });

test("health answers 200", async () => {
  assert.equal((await call("/v1/health")).status, 200);
});

test("a missing or unknown token is 401", async () => {
  assert.equal((await call("/v1/health", { token: null })).status, 401);
  assert.equal((await call("/v1/health", { token: "nope" })).status, 401);
  assert.equal((await call(`/v1/artifacts/${newKey()}`, { token: "" })).status, 401);
});

test("put then get returns the bytes, the computed hash and the echoed meta", async () => {
  const key = newKey();
  const bytes = randomBytes(1000);
  assert.equal((await put(key, bytes)).status, 200);
  const got = await call(`/v1/artifacts/${key}`);
  assert.equal(got.status, 200);
  assert.deepEqual(Buffer.from(await got.arrayBuffer()), bytes);
  assert.equal(got.headers.get("X-Cook-Content-Hash"), sha(bytes));
  for (const [name, value] of Object.entries(meta)) {
    if (name !== "X-Cook-Content-Hash") assert.equal(got.headers.get(name) ?? "", value, name);
  }
});

test("a claimed hash that does not match the body is 400, and nothing is stored", async () => {
  const key = newKey();
  const res = await put(key, "abc", { headers: { ...meta, "X-Cook-Content-Hash": "1".repeat(64) } });
  assert.equal(res.status, 400);
  assert.equal((await call(`/v1/artifacts/${key}`)).status, 404);
  assert.equal((await put(key, "abc", { headers: { ...meta, "X-Cook-Content-Hash": sha("abc") } })).status, 200);
});

test("same key with different content is 409; identical content succeeds again", async () => {
  const key = newKey();
  assert.equal((await put(key, "one")).status, 200);
  assert.equal((await put(key, "one")).status, 200);
  assert.equal((await put(key, "two")).status, 409);
  const got = await call(`/v1/artifacts/${key}`);
  assert.equal(await got.text(), "one");
});

test("racing puts of different content: exactly one wins, the rest are 409", async () => {
  const key = newKey();
  const statuses = (await Promise.all(["a", "b", "c", "d"].map((b) => put(key, b)))).map((r) => r.status).sort();
  assert.deepEqual(statuses, [200, 409, 409, 409]);
});

test("a body over 32 MiB is 413, with or without a Content-Length", async () => {
  const key = newKey();
  const big = new Uint8Array(32 * 1024 * 1024 + 1);
  assert.equal((await put(key, big)).status, 413);
  const streamed = new ReadableStream({ start(c) { c.enqueue(big.subarray(0, big.length / 2)); c.enqueue(big.subarray(big.length / 2)); c.close(); } });
  assert.equal((await put(key, streamed, { duplex: "half" })).status, 413);
  assert.equal((await call(`/v1/artifacts/${key}`)).status, 404);
  const exact = new Uint8Array(32 * 1024 * 1024);
  assert.equal((await put(newKey(), exact)).status, 200);
});

test("metadata over the R2 limit is 413 and nothing is stored", async () => {
  const key = newKey();
  const res = await put(key, "x", { headers: { ...meta, "X-Cook-Output-Path": "p".repeat(8192) } });
  assert.equal(res.status, 413);
  assert.match(await res.text(), /8192/);
  assert.equal((await call(`/v1/artifacts/${key}`)).status, 404);
});

test("a long bad batch_query key is echoed truncated", async () => {
  const res = await query(["z".repeat(100000)]);
  assert.equal(res.status, 400);
  assert.ok((await res.text()).length < 300);
});

test("identical tokens for both zones: every request is 500", async () => {
  const same = await startWorker({ trusted: "same", untrusted: "same" });
  try {
    for (const path of ["/v1/health", `/v1/artifacts/${newKey()}`]) {
      const res = await fetch(same.url + path, { headers: { Authorization: "Bearer same" } });
      assert.equal(res.status, 500);
      const body = await res.text();
      assert.match(body, /TRUSTED_TOKEN/);
      assert.ok(!body.includes("same"));
    }
  } finally { await same.stop(); }
});

test("batch_query reports a key present only when stored", async () => {
  const [a, b] = [newKey(), newKey()];
  await put(a, "x");
  const res = await query([a, b]);
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { present: [a] });
});

test("delete removes the artifact and is idempotent", async () => {
  const key = newKey();
  await put(key, "x");
  assert.equal((await call(`/v1/artifacts/${key}`, { method: "DELETE" })).status, 204);
  assert.equal((await call(`/v1/artifacts/${key}`)).status, 404);
  assert.equal((await call(`/v1/artifacts/${key}`, { method: "DELETE" })).status, 204);
});

test("manifests round-trip, are 404 when absent, and must name their URL key", async () => {
  const key = newKey();
  assert.equal((await call(`/v1/manifests/${key}`)).status, 404);
  const manifest = { key, inputs: ["a"] };
  const send = (url, m) => call(`/v1/manifests/${url}`, { method: "PUT", body: JSON.stringify(m), headers: { "Content-Type": "application/json" } });
  assert.equal((await send(key, manifest)).status, 200);
  assert.deepEqual(await (await call(`/v1/manifests/${key}`)).json(), manifest);
  const other = newKey();
  assert.equal((await send(other, manifest)).status, 400);
  assert.equal((await call(`/v1/manifests/${other}`)).status, 404);
  assert.equal((await call(`/v1/manifests/${other}`, { method: "PUT", body: "{" })).status, 400);
});

test("zones cannot see each other", async () => {
  const key = newKey();
  await put(key, "trusted-bytes");
  await call(`/v1/manifests/${key}`, { method: "PUT", body: JSON.stringify({ key }) });
  const asUntrusted = { token: w.untrusted };
  assert.equal((await call(`/v1/artifacts/${key}`, asUntrusted)).status, 404);
  assert.equal((await call(`/v1/manifests/${key}`, asUntrusted)).status, 404);
  assert.deepEqual(await (await query([key], asUntrusted)).json(), { present: [] });
  assert.equal((await put(key, "untrusted-bytes", asUntrusted)).status, 200, "no conflict across zones");
  assert.equal(await (await call(`/v1/artifacts/${key}`)).text(), "trusted-bytes");
  const other = newKey();
  await put(other, "u", asUntrusted);
  assert.equal((await call(`/v1/artifacts/${other}`)).status, 404);
  assert.deepEqual(await (await query([other])).json(), { present: [] });
  await call(`/v1/artifacts/${key}`, { method: "DELETE", ...asUntrusted });
  assert.equal((await call(`/v1/artifacts/${key}`)).status, 200, "an untrusted delete leaves trusted alone");
});

test("unrecognized input is refused with a reason", async () => {
  const key = newKey();
  assert.equal((await call("/v1/nope")).status, 404);
  assert.equal((await call(`/v1/artifacts/${key}`, { method: "POST" })).status, 405);
  assert.equal((await call("/v1/health", { method: "POST" })).status, 405);
  for (const bad of ["abc", key.toUpperCase(), `${key}00`, "..%2F..%2Funtrusted%2Fartifacts%2F" + key, `${key}/x`]) {
    const res = await call(`/v1/artifacts/${bad}`);
    assert.ok([400, 404].includes(res.status), `${bad}: ${res.status}`);
    assert.ok((await res.text()).length > 0);
  }
  assert.equal((await call(`/v1/artifacts/abc`)).status, 400);
  assert.equal((await put(key, "x", { headers: {} })).status, 400, "missing content hash");
  assert.equal((await query(["zz"])).status, 400);
  assert.equal((await call("/v1/artifacts/batch_query", { method: "POST", body: "{" })).status, 400);
  assert.equal((await call("/v1/artifacts/batch_query", { method: "POST", body: "{}" })).status, 400);
});
