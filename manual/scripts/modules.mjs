// Renders one docs page per blessed module from its README in the cook-modules checkout
// (COOK_MODULES_DIR, default ../../cook-modules). Generated into content/modules/; never edit the output.
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const root = resolve(new URL('..', import.meta.url).pathname);
const modulesDir = resolve(root, process.env.COOK_MODULES_DIR ?? '../../cook-modules');
const out = resolve(root, 'content/modules');
const names = ['cook_cc', 'cook_pnpm', 'cook_dotnet', 'cook_rust'];
const blurb = { cook_cc: 'C and C++', cook_pnpm: 'pnpm workspaces', cook_dotnet: '.NET', cook_rust: 'Cargo' };
const semver = v => v.split(/[.-]/).map(Number);
const latest = dir => readdirSync(dir, { recursive: true }).map(String).filter(f => f.endsWith('.rockspec')).map(f => f.match(/-(\d+\.\d+\.\d+-\d+)\.rockspec$/)?.[1]).filter(Boolean)
  .sort((a, b) => { const x = semver(a), y = semver(b); for (let i = 0; i < 4; i++) if (x[i] !== y[i]) return y[i] - x[i]; return 0; })[0] ?? '';
rmSync(out, { recursive: true, force: true }); mkdirSync(out, { recursive: true });
for (const name of names) {
  const readme = resolve(modulesDir, name, 'README.md');
  if (!existsSync(readme)) { console.error(`modules: no README at ${readme}`); process.exit(1); }
  const body = readFileSync(readme, 'utf8').replace(/^# .*\n+/, '');
  const summary = (body.split(/\n\s*\n/).find(p => p && !p.startsWith('```') && !p.startsWith('#') && !p.startsWith('|')) ?? '').replace(/\s+/g, ' ').replace(/[`*\[\]]/g, '').replace(/\(http[^)]*\)/g, '').slice(0, 160).replace(/\s\S*$/, '…');
  const version = latest(resolve(modulesDir, name));
  writeFileSync(`${out}/${name}.md`, `---\ntitle: "${name}"\nsubtitle: "${blurb[name]} · ${version}"\nsummary: ${JSON.stringify(summary)}\ntags: [module]\nmodule: true\n---\n\n> [!note] Source of truth\n> This page is the module's README in [LioraLabs/cook-modules](https://github.com/lioralabs/cook-modules/tree/main/${name}), rendered on every build. Latest release: [${version} on rocks.usecook.com](https://rocks.usecook.com/#${name}).\n\n${body.trim()}\n`);
}
console.log(`modules: ${names.length} pages from ${modulesDir}`);
