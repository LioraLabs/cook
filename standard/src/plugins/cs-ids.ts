import fs from 'node:fs';
import path from 'node:path';

/**
 * Extracts every `CS-NNNN` identifier from E-changes.mdx. Used to validate
 * CS-NNNN references elsewhere in the spec.
 *
 * NOTE: this harvests ALL CS-NNNN tokens from the file, including
 * back-references in prose (e.g. "supersedes CS-0001" inside a CS-0003
 * entry). A CS-NNNN that appears only in prose — never as a heading — is
 * still treated as known and will silently validate dangling references
 * elsewhere. In practice every CS-NNNN in E-changes.mdx has a heading,
 * so this is an imprecision rather than a practical problem.
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
  return path.join(projectRoot, 'src/content/docs/appendix/E-changes.mdx');
}
