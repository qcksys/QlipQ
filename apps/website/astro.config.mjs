import { defineConfig } from "astro/config";

// A static content site deployed to Cloudflare Workers via Static Assets
// (see wrangler.jsonc). No adapter is needed for static output.
export default defineConfig({
  site: "https://qlipq.app",
});
