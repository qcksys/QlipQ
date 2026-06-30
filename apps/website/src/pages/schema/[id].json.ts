import type { APIRoute, GetStaticPaths } from "astro";

// The schema describes the desktop app's config.json. Bump VERSION when the AppConfig
// shape changes; the app stamps the unversioned /schema/config.json alias, so this only
// names the pinned /schema/config-<VERSION>.json artifact.
const VERSION = "0.0.0";

function buildConfigSchema(id: string) {
  return {
    $schema: "http://json-schema.org/draft-07/schema#",
    $id: `https://qlipq.com/schema/${id}.json`,
    title: "QlipQ configuration",
    description: `Schema for QlipQ's config.json (@qcksys/qlipq-core ${VERSION}).`,
    type: "object",
    properties: {
      watchedFolders: { type: "array", items: { type: "string" } },
      outputFolder: { type: "string" },
      videoExtensions: { type: "array", items: { type: "string" } },
      namingTemplate: { type: "string" },
      ffmpegPath: { type: "string" },
      ffprobePath: { type: "string" },
      afterExport: {
        type: "object",
        properties: {
          action: { enum: ["nothing", "delete", "move", "rename", "prompt"] },
          moveFolder: { type: "string" },
          renamePrefix: { type: "string" },
          renameSuffix: { type: "string" },
        },
      },
      output: {
        type: "object",
        properties: {
          qualityMode: { enum: ["preset", "crf", "bitrate", "vbr"] },
          qualityPreset: { enum: ["original", "high", "balanced", "small"] },
          crf: { type: "integer", minimum: 0, maximum: 51 },
          videoBitrateKbps: { type: "integer", minimum: 0 },
          encoderPreset: { type: "string" },
          videoCodec: { enum: ["libx264", "libx265"] },
          container: { enum: ["mp4", "mkv"] },
          fps: { type: "integer", minimum: 0 },
          maxHeight: { type: "integer", minimum: 0 },
          audioBitrateKbps: { type: "integer", minimum: 0 },
        },
      },
    },
  };
}

// Emit both a version-pinned URL and a stable "latest" alias.
export const getStaticPaths: GetStaticPaths = () => [
  { params: { id: `config-${VERSION}` } },
  { params: { id: "config" } },
];

export const GET: APIRoute = ({ params }) =>
  new Response(JSON.stringify(buildConfigSchema(params.id ?? "config"), null, 2), {
    headers: { "content-type": "application/json" },
  });
