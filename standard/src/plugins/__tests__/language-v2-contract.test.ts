import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const steps = readFileSync(new URL('../../content/docs/08-step-kinds.mdx', import.meta.url), 'utf8');
const grammar = readFileSync(new URL('../../content/docs/appendix/A-grammar.mdx', import.meta.url), 'utf8');
const cache = readFileSync(new URL('../../content/docs/17-cache.mdx', import.meta.url), 'utf8');
const probes = readFileSync(new URL('../../content/docs/22-probe-units.mdx', import.meta.url), 'utf8');
const modules = readFileSync(new URL('../../content/docs/12-modules.mdx', import.meta.url), 'utf8');
const changes = readFileSync(new URL('../../content/docs/appendix/E-changes.mdx', import.meta.url), 'utf8');

const section = (document: string, start: string, end: string) => {
  const startAt = document.indexOf(start);
  const endAt = document.indexOf(end, startAt + start.length);
  expect(startAt, `missing section start: ${start}`).toBeGreaterThanOrEqual(0);
  expect(endAt, `missing section end: ${end}`).toBeGreaterThan(startAt);
  return document.slice(startAt, endAt);
};

const normalize = (text: string) => text.replace(/\s+/g, ' ').trim();

const expectBareGatherUnion = (summary: string) => {
  const normalized = normalize(summary);
  expect(normalized).toMatch(/bare gather source.*named `?files`?.*manifest keys.*ordinary probe.*array.*elements.*members/i);
  expect(normalized).toMatch(/non-array ordinary probe is rejected/i);
  expect(normalized).toMatch(/`?tools`? declaration is not iterable/i);
  expect(normalized).not.toMatch(/bare gather source[^.]*\b(?:is|only names?|selects only)\b[^.]*\bprobe/i);
  expect(normalized).not.toMatch(/named files cannot (?:be )?gather/i);
};

const expectSevenRuleDispatch = (dispatch: string) => {
  const rules = [...dispatch.matchAll(/^(\d+)\.\s+([^\n]+)/gm)].map((match) => [Number(match[1]), normalize(match[2])]);
  expect(rules.map(([number]) => number)).toEqual([1, 2, 3, 4, 5, 6, 7]);
  expect(rules.map(([, rule]) => rule)).toEqual([
    expect.stringMatching(/^(?:Prefix )?`gather`/),
    expect.stringMatching(/^(?:Prefix )?`seal`/),
    expect.stringMatching(/^(?:Prefix )?`unseal`/),
    expect.stringMatching(/^(?:Prefix )?`cook`/),
    expect.stringMatching(/^(?:Prefix )?`test`/),
    expect.stringMatching(/^`BARE_IDENTIFIER/),
    expect.stringMatching(/^Otherwise/),
  ]);
};

const assertContract = ({ stepsDoc = steps, grammarDoc = grammar, cacheDoc = cache, probesDoc = probes, modulesDoc = modules, changesDoc = changes } = {}) => {
    const disposition = section(stepsDoc, '### 8.4.3. Cook-step disposition', '## 8.5. `cook`');
    const testSteps = section(stepsDoc, '## 8.6. `test` step', '## 8.7.');
    const gatheredInputs = section(stepsDoc, '## 8.2. Gathered inputs', '## 8.3.');
    const appSteps = section(grammarDoc, '## A.4. Steps', '## A.5. Primitives');
    const stepsDispatch = section(stepsDoc, '## 8.1. Step-dispatch cascade', '## 8.2. Gathered inputs');
    const appDispatch = section(grammarDoc, '**Step-dispatch priority (normative).**', '## A.5. Primitives');
    const appTestModes = section(grammarDoc, '**Test mode coherence', '**`>>{` is rejected as a body.**');
    const cacheIdentity = section(cacheDoc, '#### 17.1.1.1. Effect kind', '#### 17.1.1.2.');
    const sharing = section(cacheDoc, '### 17.1.3. Sharing and disposition effect', '## 17.2.');

    const cookMods = disposition.match(/^cook_mods\s*::=([\s\S]*?)(?=^share_mod\s*::=)/m)?.[1];
    expect(cookMods && normalize(cookMods)).toBe('share_mod?');
    expect(normalize(disposition)).toContain('There is exactly one seal tier');

    const testProduction = testSteps.match(/^test_step\s*::=([\s\S]*?)(?=^body\s*::=)/m)?.[1];
    expect(testProduction && normalize(testProduction)).toBe('"test" body NEWLINE');
    expect(testSteps).not.toMatch(/\btest_mods\b/);

    const appTestProduction = grammarDoc.match(/^test_step\s*::=([\s\S]*?)(?=^body\s*::=)/m)?.[1];
    expect(appTestProduction && normalize(appTestProduction)).toBe('"test" body NEWLINE');

    const normalizedTests = normalize(testSteps);
    expect(normalizedTests).toContain('| absent | no source, non-empty recipe seal set | **single unit**, keyed on the seal set (CS-0223) |');
    expect(normalizedTests).toContain('| absent | neither source nor seal | **single unit**, uncached, runs every invocation (engine `OneShot`) |');
    expect(normalizedTests).not.toMatch(/\b(?:unit|test)[^.]*\b(?:source-less|no source)\b[^.]*\balways\b[^.]*\b(?:bypasses? (?:the )?cache|uncached|no cache key)\b/i);

    expect(gatheredInputs).not.toContain('CS-0226');
    expect(gatheredInputs).toContain('[added CS-0224]');
    for (const summary of [gatheredInputs, appSteps]) expectBareGatherUnion(summary);
    expect(cacheIdentity).not.toMatch(/test\s*\{[^}]*\}\s+seal\b/);
    expect(disposition).toContain('`cook-disposition-seal-envs-probe`');
    expect(disposition).toContain('A change to `toolchain` therefore re-runs both tests');

    expectSevenRuleDispatch(stepsDispatch);
    expectSevenRuleDispatch(appDispatch);

    const normalizedAppModes = normalize(appTestModes);
    expect(normalizedAppModes).toContain('keyed on its source set (engine `ManyToOne`), on a non-empty recipe seal set when source-less (CS-0223), or runs uncached (engine `OneShot`) only when it has neither source nor seal');
    expect(normalizedAppModes).not.toMatch(/\b(?:unit|test)[^.]*\b(?:source-less|no source)\b[^.]*\balways\b[^.]*\b(?:bypasses? (?:the )?cache|uncached|no cache key)\b/i);

    const normalizedSharing = normalize(sharing);
    expect(normalizedSharing).toContain('a declared input, a materialised data member (§{exec.cache.test-unit} rule 1), or a non-empty seal set (§{exec.cache.seal-only-key})');
    expect(normalizedSharing).not.toMatch(/\b(?:unit|test)[^.]*\b(?:source-less|no source)\b[^.]*\balways\b[^.]*\b(?:bypasses? (?:the )?cache|uncached|no cache key)\b/i);
    expect(normalizedSharing).not.toMatch(/\*\(unannotated\)\*\s*\/\s*`seal`/);
    expect(normalizedSharing).toContain('A recipe-level `seal` changes the unit\'s key, not this default sharing policy.');

    expect(changesDoc.match(/^## CS-0226\b/gm)).toHaveLength(1);
    const cs0226Heading = changesDoc.match(/^## CS-0226.*$/m)?.[0];
    expect(cs0226Heading).toBe('## CS-0226 — remove the `envs` probe producer [#changes.cs-0226]');
    expect(cs0226Heading?.match(/\bCS-0226\b/g)).toHaveLength(1);
    const v018Index = changesDoc.match(/^- \*\*v0\.18\*\*.*$/m)?.[0];
    expect(v018Index).toBeDefined();
    expect(v018Index?.match(/\bCS-0226\b/g)).toHaveLength(1);
    const cs0226 = section(changesDoc, '## CS-0226 —', '## CS-0225 —');
    expect(normalize(cs0226)).toMatch(/Removes `envs \{ NAME, … \}`.*ordinary named shell probe.*sealed by name/);
    expect(cs0226).not.toMatch(/\bgather\b/i);
    expect(cs0226.slice(cs0226.indexOf('\n')).match(/\bCS-0226\b/g)).toHaveLength(1);
    expect(changesDoc).toContain('`probe-seal` pins the probe-body position');
    expect(changesDoc).toContain('`070-seal-inline-files` pins the recipe baseline');

    const declarations = section(probesDoc, '- `files NAME`', 'The `json` and `lines` kinds');
    expect(normalize(declarations)).toContain('both a sealable determinant and a named `gather NAME` source');
    expect(normalize(declarations)).toContain('manifest keys become the gathered members');
    expect(declarations).not.toMatch(/\bseal-only\b/i);

    const memberSources = section(probesDoc, '## 22.5.10.', '## 22.5.11.');
    expectBareGatherUnion(memberSources);
    expect(normalize(memberSources)).toContain('There is no segment-count limit');
    expect(memberSources).not.toMatch(/(?:at most two|four or more segments)/i);

    const moduleKeys = section(modulesDoc, '### 12.7.6. Probe-key naming', '### 12.7.7.');
    expect(normalize(moduleKeys)).toContain('`PROBE_SEG (":" PROBE_SEG)*`, with no segment-count limit');
    expect(moduleKeys).not.toMatch(/at most two/i);

    expect(changesDoc.match(/^## CS-0230\b/gm)).toHaveLength(1);
    const cs0230 = section(changesDoc, '## CS-0230 —', '## CS-0229 —');
    expect(normalize(cs0230)).toMatch(/named `files` declarations.*`gather` sources.*manifest keys/i);
    expect(normalize(cs0230)).toMatch(/unlimited.*segment/i);
    expect(cs0230).toContain('`068-gather-named-files`');
    const versions = section(changesDoc, '## Versions', '- **v0.17**');
    expect(versions.match(/\bCS-0230\b/g)).toHaveLength(1);
};

describe('Language v2 Standard contract', () => {
  it('keeps seal recipe-level and gives CS-0226 one envs-removal entry', () => {
    assertContract();
  });

  it('rejects wrapped test-tail grammar', () => {
    const mutated = steps.replace('test_step ::= "test" body NEWLINE', 'test_step ::= "test" body\n              test_mods? NEWLINE');
    expect(() => assertContract({ stepsDoc: mutated })).toThrow();

    const appMutation = grammar.replace('test_step             ::= "test" body NEWLINE', 'test_step             ::= "test" body\n                            test_mods? NEWLINE');
    expect(() => assertContract({ grammarDoc: appMutation })).toThrow();
  });

  it('rejects a universal no-source cache bypass but permits the no-seal case', () => {
    const contradiction = steps.replace('## 8.7.', 'A unit with no source always bypasses the cache.\n\n## 8.7.');
    expect(() => assertContract({ stepsDoc: contradiction })).toThrow();

    const valid = steps.replace('## 8.7.', 'A source-less unit with no seal runs uncached.\n\n## 8.7.');
    expect(() => assertContract({ stepsDoc: valid })).not.toThrow();
  });

  it('rejects every CS-0226 attribution in gathered inputs', () => {
    const mutated = steps.replace('DAG node.', 'DAG node. [changed by CS-0226]');
    expect(() => assertContract({ stepsDoc: mutated })).toThrow();

    const entryMutation = changes.replace('## CS-0225 —', 'CS-0226 also removes gather.\n\n## CS-0225 —');
    expect(() => assertContract({ changesDoc: entryMutation })).toThrow();
  });

  it('rejects a duplicated gather arm in either dispatch copy', () => {
    const stepsMutation = steps.replace('2. Prefix `seal`', '2. Prefix `gather` + separator → `gather_step`.\n3. Prefix `seal`');
    expect(() => assertContract({ stepsDoc: stepsMutation })).toThrow();

    const appMutation = grammar.replace('2. Prefix `seal`', '2. Prefix `gather` + separator → `gather_step`.\n3. Prefix `seal`');
    expect(() => assertContract({ grammarDoc: appMutation })).toThrow();
  });

  it('rejects universal no-source cache bypasses in App A and §17.1.3', () => {
    const appMutation = grammar.replace('**`>>{` is rejected as a body.**', 'A unit with no source always bypasses the cache.\n\n**`>>{` is rejected as a body.**');
    expect(() => assertContract({ grammarDoc: appMutation })).toThrow();

    const cacheMutation = cache.replace('## 17.2.', 'A unit with no source always bypasses the cache.\n\n## 17.2.');
    expect(() => assertContract({ cacheDoc: cacheMutation })).toThrow();

    const validApp = grammar.replace('**`>>{` is rejected as a body.**', 'A source-less unit with no seal runs uncached.\n\n**`>>{` is rejected as a body.**');
    expect(() => assertContract({ grammarDoc: validApp })).not.toThrow();
  });

  it('rejects seal as a sharing disposition and a duplicated index token', () => {
    const sharingMutation = cache.replace('1. *(unannotated)* —', '1. *(unannotated)* / `seal` —');
    expect(() => assertContract({ cacheDoc: sharingMutation })).toThrow();

    const indexMutation = changes.replace('CS-0226, CS-0225', 'CS-0226, CS-0226, CS-0225');
    expect(() => assertContract({ changesDoc: indexMutation })).toThrow();
  });

  it('retains canonical citations, toolchain invalidation, and one entry-body token', () => {
    for (const [document, replacement, field] of [
      [steps, '`cook-disposition-seal-envs-probe`', 'stepsDoc'],
      [steps, '[added CS-0224]', 'stepsDoc'],
      [steps, 'A change to `toolchain` therefore re-runs both tests', 'stepsDoc'],
      [changes, '`probe-seal` pins the probe-body position', 'changesDoc'],
      [changes, '`070-seal-inline-files` pins the recipe baseline', 'changesDoc'],
    ] as const) {
      expect(() => assertContract({ [field]: document.replace(replacement, 'removed') })).toThrow();
    }

    const bodyMutation = changes.replace('CS-0226 diagnostic', 'CS-0226 CS-0226 diagnostic');
    expect(() => assertContract({ changesDoc: bodyMutation })).toThrow();

    for (const suffix of [' CS-0226', ' (CS-0226)']) {
      const headingMutation = changes.replace('producer [#changes.cs-0226]', `producer${suffix} [#changes.cs-0226]`);
      expect(() => assertContract({ changesDoc: headingMutation })).toThrow();
    }
  });

  it('pins named-files gather, unlimited source refs, and one CS-0230 entry', () => {
    expect(() => assertContract({ probesDoc: probes.replace('both a sealable determinant and a named `gather NAME` source', 'seal-only') })).toThrow();
    expect(() => assertContract({ probesDoc: probes.replace('There is no segment-count limit', 'A source ref of four or more segments is malformed') })).toThrow();
    expect(() => assertContract({ modulesDoc: modules.replace('with no segment-count limit', 'with at most two segments') })).toThrow();
    expect(() => assertContract({ changesDoc: changes.replace('## CS-0229 —', '## CS-0230 — duplicate\n\n## CS-0229 —') })).toThrow();
    expect(() => assertContract({ changesDoc: changes.replace(/## CS-0230 —[\s\S]*?(?=## CS-0229 —)/, '') })).toThrow();
  });

  it('rejects probe-only or named-files-denial wording in every bare-gather summary', () => {
    for (const [document, anchor, field] of [
      [steps, '### Note 8.2.1 — overload rule', 'stepsDoc'],
      [grammar, '## A.4. Steps', 'grammarDoc'],
      [probes, '## 22.5.10.', 'probesDoc'],
    ] as const) {
      const probeOnly = document.replace(anchor, `${anchor}\n\nA bare gather source is only an array-valued probe.`);
      expect(() => assertContract({ [field]: probeOnly })).toThrow();

      const denied = document.replace(anchor, `${anchor}\n\nNamed files cannot gather.`);
      expect(() => assertContract({ [field]: denied })).toThrow();
    }
  });
});
