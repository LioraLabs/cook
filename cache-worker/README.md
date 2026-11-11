# cache-worker

A Cloudflare Worker that serves cook's v1 remote-cache protocol over an R2 bucket.

## Routes

Every route, health included, needs `Authorization: Bearer <token>`.

- `GET /v1/health`
- `GET`, `PUT`, `DELETE /v1/artifacts/<key>`
- `POST /v1/artifacts/batch_query`
- `GET`, `PUT /v1/manifests/<key>`

Keys are 64 lowercase hex characters. A PUT of different bytes under an existing key is a 409; identical bytes succeed again.

## Zones and tokens

The bearer token picks the zone; objects live under `trusted/` or `untrusted/` in the bucket and neither zone can read the other.

- `TRUSTED_TOKEN`: the owner's machines.
- `UNTRUSTED_TOKEN`: builds of untrusted input, for example the ROM builder container.

Both are Worker secrets and must differ; equal tokens make every request a 500.

## Deploy

From scratch:

```sh
npx wrangler r2 bucket create cook-cache
npx wrangler r2 bucket lifecycle add cook-cache expire-30d --expire-days 30
npx wrangler deploy
openssl rand -hex 32 | npx wrangler secret put TRUSTED_TOKEN
openssl rand -hex 32 | npx wrangler secret put UNTRUSTED_TOKEN
```

The lifecycle rule is the only eviction: an expired entry is a miss and gets republished. Until both secrets are set every request is a 401.

## Limits

- Artifact: 32 MiB (413 above).
- `batch_query`: 1000 keys per request.
- Manifest: 4 MiB.
- Artifact metadata (the `X-Cook-*` headers, mainly output path and symlink target): 8192 bytes, R2's object metadata limit (413 above).

## Local development

`pnpm install`, then `pnpm test`. `test/start-worker.js` exports `startWorker()`, which runs the Worker against a temporary local R2 and returns `{ url, trusted, untrusted, stop }`. `test/cook-e2e.test.js` drives the real `cook` binary (`COOK_BIN`, default `cook` on PATH) and is skipped with a message if it cannot run, unless `COOK_BIN` is set, in which case it fails. The root Cookfile's `cache-worker` recipe runs the suite against the cook built in this repo.

## Client config

A consumer's `.cook/cloud.toml`, with the zone's token in `COOK_CLOUD_API_KEY`:

```toml
[cloud]
enabled = true
project = "my-project"
endpoint = "https://<worker-host>"
max_artifact_mib = 32
# short-lived containers: fail fast instead of hanging on a slow store
timeout_secs = 10
max_retries = 1
```

`max_artifact_mib = 32` keeps the client from attempting uploads the Worker would refuse.
