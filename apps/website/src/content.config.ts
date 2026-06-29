import { glob } from "astro/loaders";
import { defineCollection, z } from "astro:content";

// Guide pages are authored as Markdown and rendered at build time (Content Layer API).
const guide = defineCollection({
  loader: glob({ pattern: "**/*.md", base: "./src/content/guide" }),
  schema: z.object({
    title: z.string(),
    description: z.string().optional(),
    order: z.number().optional(),
  }),
});

export const collections = { guide };
