import fs from 'node:fs';
import path from 'node:path';

/**
 * Extracts every `CS-NNNN` identifier from D-changes.mdx. Used to validate
 * CS-NNNN references elsewhere in the spec.
 */
export function harvestCsIds(changesPath: string): Set<string> {
  const src = fs.readFileSync(changesPath, 'utf8');
  const ids = new Set<string>();
  const re = /\bCS-[0-9]{4}\b/g;
  for (const m of src.matchAll(re)) {
    ids.add(m[0]);
  }
  return ids;
}

export function defaultChangesPath(projectRoot: string): string {
  return path.join(projectRoot, 'src/content/docs/appendix/D-changes.mdx');
}
