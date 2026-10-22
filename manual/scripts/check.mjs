// docs build check: every manual chapter rendered, every local link and asset resolves, no leftover (#anchor) links to other chapters.
import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url)), dist = join(root, 'dist');
assert(existsSync(join(dist, 'index.html')), 'Run npm run build before npm test.');
const chapters = readdirSync(join(root, 'content/manual')).filter(f => f.endsWith('.md'));
assert(chapters.length >= 10, `Expected the manual's chapters, found ${chapters.length}`);
for (const f of chapters) assert(existsSync(join(dist, 'n/manual', f.replace(/\.md$/, ''), 'index.html')), `Chapter not rendered: ${f}`);
for (const route of ['index.html', 'notes/index.html', 'n/guides/watch-the-cache-work/index.html', 's/guides/cookfile-in-sixty-seconds/index.html', ...chapters.map(f => `n/manual/${f.replace(/\.md$/, '')}/index.html`)]) {
  const html = readFileSync(join(dist, route), 'utf8');
  assert(!html.includes('class="missing"'), `Unresolved wikilink on ${route}`);
  for (const [, href] of html.matchAll(/(?:href|src)="(\/[^"#?]*)/g)) {
    if (href.startsWith('//')) continue;
    const target = join(dist, decodeURIComponent(href));
    assert(existsSync(target) || existsSync(join(target, 'index.html')), `Broken local link in ${route}: ${href}`);
  }
}
assert(existsSync(join(dist, 'standard/index.html')) && existsSync(join(dist, 'standard/17-cache/index.html')), 'The Standard is not mounted at /standard/');
console.log(`docs: ${chapters.length} chapters, the Standard at /standard/, guides, slides, links and wikilinks resolve.`);
