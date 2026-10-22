import { defineCollection } from 'astro:content';
import { z } from 'astro/zod';
import { glob } from 'astro/loaders';

// A note is content/<name>.md(x) or content/<name>/index.md(x). content/manual/ is generated from ../document.md.
const notes = defineCollection({
  loader: glob({
    pattern: ['**/*.{md,mdx}'],
    base: './content',
    generateId: ({ entry }) => entry.replace(/\.mdx?$/, '').replace(/\/index$/, ''),
  }),
  schema: z.object({
    title: z.string().optional(),
    date: z.coerce.date().optional(),
    summary: z.string().optional(),
    tags: z.array(z.string()).default([]),
    subtitle: z.string().optional(),
    slides: z.boolean().default(false),
    order: z.number().optional(), // manual chapters read in this order
  }).passthrough(),
});

export const collections = { notes };
