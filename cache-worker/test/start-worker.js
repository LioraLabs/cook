// Starts the Worker on a free localhost port against a throwaway local R2, with known bearer tokens.
// No network and no Cloudflare login: `const w = await startWorker(); w.url; w.trusted; w.untrusted; await w.stop()`.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unstable_dev } from "wrangler";

export async function startWorker({ trusted = "trusted-test-token", untrusted = "untrusted-test-token" } = {}) {
  const persist = mkdtempSync(join(tmpdir(), "cook-cache-worker-"));
  const worker = await unstable_dev(new URL("../src/index.js", import.meta.url).pathname, {
    config: new URL("../wrangler.jsonc", import.meta.url).pathname,
    local: true,
    port: 0,
    persistTo: persist,
    vars: { TRUSTED_TOKEN: trusted, UNTRUSTED_TOKEN: untrusted },
    logLevel: "error",
    experimental: { disableExperimentalWarning: true, disableDevRegistry: true },
  });
  return {
    url: `http://${worker.address}:${worker.port}`,
    trusted,
    untrusted,
    async stop() {
      await worker.stop();
      rmSync(persist, { recursive: true, force: true });
    },
  };
}
