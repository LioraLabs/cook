// cook's v1 remote-cache protocol (cli/crates/cook-cache: cloud_backend.rs is the client, serve.rs the reference
// server) over one R2 bucket. A bearer token selects a zone; a zone owns the key prefix `<zone>/` and sees nothing else.

/// A whole artifact is buffered to hash it, so the cap is a memory bound as well as a policy.
const MAX_ARTIFACT_BYTES = 32 * 1024 * 1024;
const MAX_MANIFEST_BYTES = 4 * 1024 * 1024;
/// Each key is one R2 head, and a Worker invocation may make 1000 R2 calls.
const MAX_QUERY_KEYS = 1000;
const MAX_QUERY_BODY = 16 * 1024 * 1024;
/// R2's object metadata limit; keys and values are counted together because the docs do not say which is meant.
const MAX_METADATA_BYTES = 8192;

const ZERO_HASH = "0".repeat(64);
const KEY = /^[0-9a-f]{64}$/;
const META_HEADERS = [
  ["contentHash", "X-Cook-Content-Hash"],
  ["sizeBytes", "X-Cook-Size-Bytes"],
  ["schemaVersion", "X-Cook-Schema-Version"],
  ["recipeNamespace", "X-Cook-Recipe-Namespace"],
  ["outputIndex", "X-Cook-Output-Index"],
  ["outputPath", "X-Cook-Output-Path"],
  ["mode", "X-Cook-Mode"],
  ["kind", "X-Cook-Kind"],
  ["symlinkTarget", "X-Cook-Symlink-Target"],
];

const text = (status, body, headers) => new Response(body, { status, headers });

function hex(bytes) {
  return [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// Equal strings, compared over their SHA-256 digests so neither length nor content leaks through timing.
async function same(a, b) {
  const [x, y] = await Promise.all([a, b].map(async (s) => new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)))));
  let diff = 0;
  for (let i = 0; i < x.length; i++) diff |= x[i] ^ y[i];
  return diff === 0;
}

/// The zone this request's bearer token belongs to, or null. A zone with no token configured never matches.
async function zoneOf(request, env) {
  const got = request.headers.get("Authorization") ?? "";
  const zones = [["trusted", env.TRUSTED_TOKEN], ["untrusted", env.UNTRUSTED_TOKEN]];
  for (const [zone, token] of zones) {
    if (token && (await same(got, `Bearer ${token}`))) return zone;
  }
  return null;
}

/// The body, refused (null) as soon as it passes `limit` bytes; never buffers more than limit plus one chunk.
async function readCapped(request, limit) {
  const declared = request.headers.get("Content-Length");
  if (declared !== null && Number(declared) > limit) return null;
  const chunks = [];
  let total = 0;
  if (request.body) {
    const reader = request.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.length;
      if (total > limit) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

async function putArtifact(request, env, object) {
  const claimed = request.headers.get("X-Cook-Content-Hash");
  if (claimed === null || !KEY.test(claimed)) return text(400, "X-Cook-Content-Hash must be 64 lowercase hex characters");
  const body = await readCapped(request, MAX_ARTIFACT_BYTES);
  if (body === null) return text(413, `artifact exceeds ${MAX_ARTIFACT_BYTES} bytes`);
  const actual = hex(await crypto.subtle.digest("SHA-256", body));
  if (claimed !== ZERO_HASH && claimed !== actual) return text(400, `X-Cook-Content-Hash ${claimed} does not match the body (${actual})`);
  const customMetadata = {};
  for (const [field, header] of META_HEADERS) customMetadata[field] = field === "contentHash" ? actual : (request.headers.get(header) ?? "");
  const metaBytes = Object.entries(customMetadata).reduce((n, kv) => n + kv.reduce((m, x) => m + new TextEncoder().encode(x).length, 0), 0);
  if (metaBytes > MAX_METADATA_BYTES) return text(413, `X-Cook-* metadata is ${metaBytes} bytes, over the ${MAX_METADATA_BYTES} byte object metadata limit`);
  // One conditional write: only the first of racing puts lands, so a loser compares against the winner.
  const stored = await env.CACHE.put(object, body, { customMetadata, onlyIf: { etagDoesNotMatch: "*" } });
  if (stored !== null) return text(200, "stored");
  const existing = await env.CACHE.head(object);
  if (existing === null) return text(503, "store refused the write and then lost the object; retry");
  if (existing.customMetadata?.contentHash === actual) return text(200, "stored");
  return text(409, `artifact key conflict: existing content_hash ${existing.customMetadata?.contentHash} differs from new bytes ${actual}`);
}

async function getArtifact(env, object) {
  const found = await env.CACHE.get(object);
  if (found === null) return text(404, "not found");
  const headers = new Headers();
  for (const [field, header] of META_HEADERS) headers.set(header, found.customMetadata?.[field] ?? "");
  return text(200, found.body, headers);
}

async function batchQuery(request, env, zone) {
  const body = await readCapped(request, MAX_QUERY_BODY);
  if (body === null) return text(413, "batch_query body too large");
  let keys;
  try {
    keys = JSON.parse(new TextDecoder().decode(body)).keys;
  } catch (e) {
    return text(400, `malformed batch_query body: ${e.message}`);
  }
  if (!Array.isArray(keys)) return text(400, 'malformed batch_query body: "keys" must be an array');
  const bad = keys.find((k) => typeof k !== "string" || !KEY.test(k));
  if (bad !== undefined) return text(400, `key ${JSON.stringify(String(bad).slice(0, 80))} is not 64 lowercase hex characters`);
  if (keys.length > MAX_QUERY_KEYS) return text(413, `batch_query takes at most ${MAX_QUERY_KEYS} keys, got ${keys.length}`);
  const heads = await Promise.all(keys.map((k) => env.CACHE.head(`${zone}/artifacts/${k}`)));
  return text(200, JSON.stringify({ present: keys.filter((_, i) => heads[i] !== null) }), { "Content-Type": "application/json" });
}

async function putManifest(request, env, object, key) {
  const body = await readCapped(request, MAX_MANIFEST_BYTES);
  if (body === null) return text(413, `manifest over ${MAX_MANIFEST_BYTES} bytes`);
  let manifest;
  try {
    manifest = JSON.parse(new TextDecoder().decode(body));
  } catch (e) {
    return text(400, `malformed manifest: ${e.message}`);
  }
  if (manifest === null || typeof manifest !== "object" || manifest.key !== key) {
    return text(400, `manifest names key ${manifest?.key} but was addressed to ${key}`);
  }
  // Last writer wins, as in serve.rs: a manifest is a per-run observation.
  await env.CACHE.put(object, body, { httpMetadata: { contentType: "application/json" } });
  return text(200, "stored");
}

async function getManifest(env, object) {
  const found = await env.CACHE.get(object);
  return found === null ? text(404, "not found") : text(200, found.body, { "Content-Type": "application/json" });
}

async function route(request, env) {
  if (env.TRUSTED_TOKEN && env.TRUSTED_TOKEN === env.UNTRUSTED_TOKEN) return text(500, "misconfigured: TRUSTED_TOKEN and UNTRUSTED_TOKEN must differ");
  const zone = await zoneOf(request, env);
  if (zone === null) return text(401, "missing or wrong bearer token");
  const { pathname } = new URL(request.url);
  const method = request.method;
  if (pathname === "/v1/health") return method === "GET" ? text(200, "ok") : text(405, "use GET");
  if (pathname === "/v1/artifacts/batch_query") return method === "POST" ? batchQuery(request, env, zone) : text(405, "use POST");
  const m = /^\/v1\/(artifacts|manifests)\/([^/]*)$/.exec(pathname);
  if (m === null) return text(404, "unknown path");
  const [, kind, key] = m;
  if (!KEY.test(key)) return text(400, "key must be 64 lowercase hex characters");
  const object = `${zone}/${kind}/${key}`;
  if (kind === "manifests") {
    if (method === "GET") return getManifest(env, object);
    return method === "PUT" ? putManifest(request, env, object, key) : text(405, "use GET or PUT");
  }
  if (method === "GET") return getArtifact(env, object);
  if (method === "PUT") return putArtifact(request, env, object);
  if (method === "DELETE") {
    await env.CACHE.delete(object);
    return text(204, null);
  }
  return text(405, "use GET, PUT or DELETE");
}

export default {
  async fetch(request, env) {
    try {
      return await route(request, env);
    } catch (e) {
      // The client only sees the status (and retries 5xx); the reason goes to the Worker's log.
      console.error(`${request.method} ${new URL(request.url).pathname}: ${e?.stack ?? e}`);
      return text(500, "internal error");
    }
  },
};
