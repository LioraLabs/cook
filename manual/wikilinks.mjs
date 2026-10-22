// [[note]] / [[note|label]] -> link to /n/<id>. Resolves by filename or title slug against NOTES_DIR; unresolved links get class "missing".
import { readdirSync, readFileSync } from 'node:fs';
export const slug = s => s.normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
export const dir = new URL('./content', import.meta.url).pathname;

// ponytail: rescans the notes dir per document; fine for thousands of notes, cache if it ever drags.
function index() {
  const map = new Map();
  for (const f of readdirSync(dir, { recursive: true })) {
    if (!/\.mdx?$/.test(f)) continue;
    const id = f.replace(/\.mdx?$/, '').replace(/\/index$/, '');
    map.set(slug(id.split('/').pop()), id);
    const t = readFileSync(`${dir}/${f}`, 'utf8').match(/^title:\s*["']?(.+?)["']?\s*$/m);
    if (t) map.set(slug(t[1]), id);
  }
  return map;
}

export default function remarkWikilinks() {
  return tree => {
    const ids = index();
    const walk = node => {
      if (!node.children || node.type === 'link' || node.type === 'code' || node.type === 'inlineCode') return;
      node.children = node.children.flatMap(c => {
        if (c.type !== 'text' || !c.value.includes('[[')) { walk(c); return [c]; }
        const out = []; let last = 0;
        for (const m of c.value.matchAll(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g)) {
          if (m.index > last) out.push({ type: 'text', value: c.value.slice(last, m.index) });
          const id = ids.get(slug(m[1]));
          out.push({ type: 'link', url: id ? `/n/${id}` : '#', data: { hProperties: id ? {} : { className: ['missing'], title: 'no such note' } }, children: [{ type: 'text', value: (m[2] ?? m[1]).trim() }] });
          last = m.index + m[0].length;
        }
        if (last < c.value.length) out.push({ type: 'text', value: c.value.slice(last) });
        return out;
      });
    };
    walk(tree);
  };
}
