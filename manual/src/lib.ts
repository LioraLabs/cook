import { getCollection, type CollectionEntry } from 'astro:content';

export type Note = CollectionEntry<'notes'>;
export const titleOf = (n: Note) => n.data.title ?? n.id.split('/').pop()!.replace(/[-_]+/g, ' ');
export const when = (n: Note) => n.data.date?.getTime() ?? 0;
export const fmt = (d?: Date) => d ? new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC' }).format(d) : '';
export const getNotes = async () => (await getCollection('notes')).sort((a, b) => when(b) - when(a) || a.id.localeCompare(b.id));
/** The manual's chapters, in reading order. */
export const getManual = async () => (await getCollection('notes')).filter(n => n.data.order).sort((a, b) => a.data.order! - b.data.order!);

import { slug, dir } from '../wikilinks.mjs';
export { dir };
/** Notes that link to `note` with [[...]] (by filename or title). */
export const backlinks = (note: Note, all: Note[]) => {
  const keys = new Set([slug(note.id.split('/').pop()!), slug(titleOf(note))]);
  return all.filter(o => o.id !== note.id && [...(o.body ?? '').matchAll(/\[\[([^\]|]+)/g)].some(m => keys.has(slug(m[1]))));
};
