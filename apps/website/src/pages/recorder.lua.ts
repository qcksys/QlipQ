import recorderLua from "@qcksys/qlipq-obs-script/recorder.lua?raw";
import type { APIRoute } from "astro";

// Serves the OBS companion script at /recorder.lua, sourced directly from the
// @qcksys/qlipq-obs-script package (single source of truth — no copy in public/).
export const GET: APIRoute = () =>
  new Response(recorderLua, {
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
