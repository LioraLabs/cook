// Splits ../document.md (THE manual, one source of truth) into one note per chapter under content/manual/.
// Runs before dev and build. Never edit the output; edit document.md.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const src = readFileSync(resolve(root, '../document.md'), 'utf8');
const out = resolve(root, 'content/manual');
const slug = s => s.toLowerCase().replace(/[`*]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
const github = 'https://github.com/LioraLabs/cook/blob/main/';

// Chapters are the `## ` sections after "Contents"; the preamble feeds the landing page.
const lines = src.split('\n');
const chapters = []; let cur = null; let preamble = [];
for (const line of lines) {
  const h2 = line.match(/^## (.+)$/);
  if (h2) { cur = { title: h2[1], body: [] }; if (h2[1] !== 'Contents') chapters.push(cur); continue; }
  if (!cur) { if (!/^# /.test(line)) preamble.push(line); continue; }
  cur.body.push(line);
}
chapters.forEach((c, i) => { c.n = i + 1; c.id = `manual/${String(c.n).padStart(2, '0')}-${slug(c.title)}`; });
// Every heading anchor in the manual maps to the chapter that holds it, so `(#anchor)` links keep working across notes.
const anchors = new Map(chapters.map(c => [slug(c.title), `/n/${c.id}`]));
for (const c of chapters) for (const l of c.body) { const h = l.match(/^#{3,4} (.+)$/); if (h) anchors.set(slug(h[1]), `/n/${c.id}#${slug(h[1])}`); }

const rewrite = text => text
  .replace(/\]\(#([a-z0-9-]+)\)/g, (m, a) => anchors.has(a) ? `](${anchors.get(a)})` : m)
  .replace(/\]\(standard\/\)/g, '](https://standard.usecook.com/)')
  .replace(/\]\(examples\/\)/g, `](${github}examples/)`)
  .replace(/\]\((docs\/[^)]+\.md)\)/g, `](${github}$1)`)
  .replace(/^### /gm, '## ').replace(/^#### /gm, '### ');

// Cookfile fences in the manual carry no language; tag the ones that look like one so they highlight.
// Walks the lines so an indented fence in a list, or one right after another fence, is paired correctly.
const tagFences = text => {
  const lines = text.split('\n'); let open = -1, lang = '';
  lines.forEach((l, i) => {
    const m = l.match(/^(\s*)```(\w*)\s*$/);
    if (!m) return;
    if (open < 0) { open = i; lang = m[2]; return; }
    if (!lang && /^\s*(recipe|chore|probe|files|tools|use|test|config)\b/m.test(lines.slice(open + 1, i).join('\n'))) lines[open] += 'cook';
    open = -1;
  });
  return lines.join('\n');
};
// The landing page shows the manual's opening paragraph as HTML: links and code only.
const inlineHtml = md => md.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>').replace(/`([^`]+)`/g, '<code>$1</code>');

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
for (const c of chapters) {
  const body = tagFences(rewrite(c.body.join('\n'))).trim();
  const summary = (body.split('\n\n').find(p => p && !p.startsWith('```') && !p.startsWith('#') && !p.startsWith('|')) ?? '').replace(/\s+/g, ' ').replace(/[`*\[\]]/g, '').replace(/\(http[^)]*\)|\(\/n\/[^)]*\)|\(#[^)]*\)/g, '').slice(0, 160).replace(/\s\S*$/, '…');
  writeFileSync(`${out}/${c.id.split('/').pop()}.md`, `---\ntitle: ${JSON.stringify(`${c.n}. ${c.title}`)}\nsummary: ${JSON.stringify(summary)}\ntags: [manual]\norder: ${c.n}\n---\n\n${body}\n`);
}
writeFileSync(resolve(root, 'content/manual/preamble.json'), JSON.stringify({ lede: inlineHtml(rewrite(preamble.join('\n')).trim()) }));
console.log(`manual: ${chapters.length} chapters from document.md`);
