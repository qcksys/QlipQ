import {
  type AfterExportAction,
  type AppConfig,
  type ContainerFormat,
  formatBytes,
  type OutputSettings,
  type QualityMode,
  type QualityPreset,
  type VideoCodecChoice,
} from "@qcksys/qlipq-core";
import { type ReactNode, useEffect, useState } from "react";
import type { CapturePresets } from "../lib/api.ts";
import * as api from "../lib/api.ts";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const ENCODER_PRESETS = [
  "ultrafast",
  "superfast",
  "veryfast",
  "faster",
  "fast",
  "medium",
  "slow",
  "slower",
  "veryslow",
];

function clampInt(value: string, min: number, max: number): number {
  const n = Math.round(Number(value));
  if (Number.isNaN(n)) return min;
  return Math.min(max, Math.max(min, n));
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 rounded-xl border border-border bg-card p-4">
      <h3 className="text-sm font-semibold">{title}</h3>
      {children}
    </section>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-1 flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

interface ConfigPanelProps {
  config: AppConfig;
  presets: CapturePresets;
  onChange: (patch: Partial<AppConfig>) => void;
  onReprocess: (folder: string) => void;
}

interface BinaryStatus {
  ok: boolean;
  message: string;
}

export function ConfigPanel({ config, presets, onChange, onReprocess }: ConfigPanelProps) {
  const [tests, setTests] = useState<{ ffmpeg?: BinaryStatus; ffprobe?: BinaryStatus }>({});
  const [cacheBytes, setCacheBytes] = useState<number | null>(null);
  const [clearing, setClearing] = useState(false);

  useEffect(() => {
    api.proxyCacheSize().then(setCacheBytes).catch(console.error);
  }, []);

  const clearCache = async () => {
    setClearing(true);
    try {
      await api.clearProxyCache();
      setCacheBytes(await api.proxyCacheSize());
    } catch (err) {
      console.error("clear preview cache failed", err);
    } finally {
      setClearing(false);
    }
  };

  const runTest = async (which: "ffmpeg" | "ffprobe", path: string) => {
    setTests((t) => ({ ...t, [which]: { ok: true, message: "Testing…" } }));
    try {
      const version = await api.checkBinary(path);
      setTests((t) => ({ ...t, [which]: { ok: true, message: version || "OK" } }));
    } catch (err) {
      setTests((t) => ({ ...t, [which]: { ok: false, message: String(err) } }));
    }
  };
  const addWatchedFolder = (folder: string) => {
    if (!config.watchedFolders.includes(folder)) {
      onChange({ watchedFolders: [...config.watchedFolders, folder] });
    }
  };

  const addFolder = async () => {
    const folder = await api.pickFolder();
    if (folder) addWatchedFolder(folder);
  };

  const removeFolder = (folder: string) => {
    onChange({ watchedFolders: config.watchedFolders.filter((f) => f !== folder) });
  };

  const presetOptions: Array<{ label: string; folder: string }> = [
    ...(presets.obs ? [{ label: "OBS", folder: presets.obs }] : []),
    ...(presets.nvidiaShare ? [{ label: "NVIDIA Share", folder: presets.nvidiaShare }] : []),
  ].filter((p) => !config.watchedFolders.includes(p.folder));

  const out = config.output;
  const setOut = (patch: Partial<OutputSettings>) => onChange({ output: { ...out, ...patch } });

  const ae = config.afterExport;
  const setAfter = (patch: Partial<typeof ae>) => onChange({ afterExport: { ...ae, ...patch } });

  const pickOutput = async () => {
    const folder = await api.pickFolder();
    if (folder) onChange({ outputFolder: folder });
  };

  const pickMoveFolder = async () => {
    const folder = await api.pickFolder();
    if (folder) setAfter({ moveFolder: folder });
  };

  const openConfigFile = async () => {
    try {
      // Ensure the file exists (auto-save may not have flushed yet), then reveal it.
      await api.setConfig(config);
      await api.revealInExplorer(await api.getConfigPath());
    } catch (err) {
      console.error("open config failed", err);
    }
  };

  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 py-2">
      <Section title="Watched folders">
        <p className="text-xs text-muted-foreground">
          New recordings in these folders (and subfolders) are added to the queue.
        </p>
        <ul className="flex flex-col gap-1.5">
          {config.watchedFolders.map((folder) => (
            <li
              key={folder}
              className="flex items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-2"
            >
              <span className="truncate text-sm" title={folder}>
                {folder}
              </span>
              <span className="flex shrink-0 gap-3">
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0"
                  onClick={() => onReprocess(folder)}
                >
                  Reprocess
                </Button>
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0"
                  onClick={() => removeFolder(folder)}
                >
                  Remove
                </Button>
              </span>
            </li>
          ))}
          {config.watchedFolders.length === 0 && (
            <li className="text-sm text-muted-foreground">None yet.</li>
          )}
        </ul>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={addFolder}>
            Add folder…
          </Button>
          {presetOptions.map((preset) => (
            <Button
              key={preset.folder}
              variant="outline"
              size="sm"
              title={preset.folder}
              onClick={() => addWatchedFolder(preset.folder)}
            >
              + {preset.label} ({preset.folder})
            </Button>
          ))}
        </div>
      </Section>

      <Section title="Output folder">
        <div className="flex gap-2">
          <Input
            value={config.outputFolder}
            placeholder="Where exported clips are saved"
            onChange={(e) => onChange({ outputFolder: e.target.value })}
          />
          <Button variant="outline" onClick={pickOutput}>
            Browse…
          </Button>
        </div>
      </Section>

      <Section title="Output defaults">
        <p className="text-xs text-muted-foreground">
          Applied to every export. The editor shows an estimated file size per clip.
        </p>

        <div className="flex flex-wrap gap-3">
          <Field label="Quality">
            <Select
              value={out.qualityMode}
              onValueChange={(v) => setOut({ qualityMode: v as QualityMode })}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="preset">Preset</SelectItem>
                <SelectItem value="crf">Custom quality (CRF)</SelectItem>
                <SelectItem value="vbr">VBR (quality + max bitrate)</SelectItem>
                <SelectItem value="bitrate">Target bitrate</SelectItem>
              </SelectContent>
            </Select>
          </Field>

          {out.qualityMode === "preset" && (
            <Field label="Preset">
              <Select
                value={out.qualityPreset}
                onValueChange={(v) => setOut({ qualityPreset: v as QualityPreset })}
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="original">Original — no re-encode</SelectItem>
                  <SelectItem value="high">High</SelectItem>
                  <SelectItem value="balanced">Balanced</SelectItem>
                  <SelectItem value="small">Small</SelectItem>
                </SelectContent>
              </Select>
            </Field>
          )}

          {(out.qualityMode === "crf" || out.qualityMode === "vbr") && (
            <Field label="CRF (0–51, lower is better)">
              <Input
                type="number"
                min={0}
                max={51}
                value={out.crf}
                onChange={(e) => setOut({ crf: clampInt(e.target.value, 0, 51) })}
              />
            </Field>
          )}

          {(out.qualityMode === "bitrate" || out.qualityMode === "vbr") && (
            <Field
              label={out.qualityMode === "vbr" ? "Max bitrate (kbps)" : "Video bitrate (kbps)"}
            >
              <Input
                type="number"
                min={100}
                step={500}
                value={out.videoBitrateKbps}
                onChange={(e) =>
                  setOut({ videoBitrateKbps: clampInt(e.target.value, 100, 200000) })
                }
              />
            </Field>
          )}
        </div>

        <div className="flex flex-wrap gap-3">
          <Field label="Encoder speed">
            <Select
              value={out.encoderPreset}
              onValueChange={(v) => {
                if (v != null) setOut({ encoderPreset: v });
              }}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {ENCODER_PRESETS.map((p) => (
                  <SelectItem key={p} value={p}>
                    {p}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>

          <Field label="Codec">
            <Select
              value={out.videoCodec}
              onValueChange={(v) => setOut({ videoCodec: v as VideoCodecChoice })}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="libx264">H.264</SelectItem>
                <SelectItem value="libx265">H.265 (smaller, slower)</SelectItem>
              </SelectContent>
            </Select>
          </Field>

          <Field label="Container">
            <Select
              value={out.container}
              onValueChange={(v) => setOut({ container: v as ContainerFormat })}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="mp4">mp4</SelectItem>
                <SelectItem value="mkv">mkv</SelectItem>
              </SelectContent>
            </Select>
          </Field>
        </div>

        <div className="flex flex-wrap gap-3">
          <Field label="Frame rate">
            <Select value={String(out.fps)} onValueChange={(v) => setOut({ fps: Number(v) })}>
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">Source</SelectItem>
                <SelectItem value="60">60</SelectItem>
                <SelectItem value="30">30</SelectItem>
              </SelectContent>
            </Select>
          </Field>

          <Field label="Resolution">
            <Select
              value={String(out.maxHeight)}
              onValueChange={(v) => setOut({ maxHeight: Number(v) })}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">Source</SelectItem>
                <SelectItem value="2160">4K (2160p)</SelectItem>
                <SelectItem value="1440">1440p</SelectItem>
                <SelectItem value="1080">1080p</SelectItem>
                <SelectItem value="720">720p</SelectItem>
              </SelectContent>
            </Select>
          </Field>

          <Field label="Audio bitrate">
            <Select
              value={String(out.audioBitrateKbps)}
              onValueChange={(v) => setOut({ audioBitrateKbps: Number(v) })}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="128">128 kbps</SelectItem>
                <SelectItem value="192">192 kbps</SelectItem>
                <SelectItem value="256">256 kbps</SelectItem>
              </SelectContent>
            </Select>
          </Field>
        </div>
      </Section>

      <Section title="Naming template">
        <Input
          value={config.namingTemplate}
          onChange={(e) => onChange({ namingTemplate: e.target.value })}
        />
        <p className="text-xs text-muted-foreground">
          Tokens: <code>{"{date}"}</code> <code>{"{time}"}</code> <code>{"{datetime}"}</code>{" "}
          <code>{"{source}"}</code> <code>{"{name}"}</code> <code>{"{index}"}</code>
        </p>
      </Section>

      <Section title="FFmpeg">
        <p className="text-xs text-muted-foreground">
          QlipQ needs <code>ffmpeg</code> and <code>ffprobe</code> on your PATH (or set full paths
          below). On Windows: <code>winget install Gyan.FFmpeg</code>, then restart QlipQ.
        </p>
        <div className="flex items-end gap-2">
          <Field label="ffmpeg path">
            <Input
              value={config.ffmpegPath}
              onChange={(e) => onChange({ ffmpegPath: e.target.value })}
            />
          </Field>
          <Button variant="outline" size="sm" onClick={() => runTest("ffmpeg", config.ffmpegPath)}>
            Test
          </Button>
        </div>
        {tests.ffmpeg && (
          <p
            className={`text-xs ${tests.ffmpeg.ok ? "text-muted-foreground" : "text-destructive"}`}
          >
            {tests.ffmpeg.ok ? "✓ " : "✗ "}
            {tests.ffmpeg.message}
          </p>
        )}
        <div className="flex items-end gap-2">
          <Field label="ffprobe path">
            <Input
              value={config.ffprobePath}
              onChange={(e) => onChange({ ffprobePath: e.target.value })}
            />
          </Field>
          <Button
            variant="outline"
            size="sm"
            onClick={() => runTest("ffprobe", config.ffprobePath)}
          >
            Test
          </Button>
        </div>
        {tests.ffprobe && (
          <p
            className={`text-xs ${tests.ffprobe.ok ? "text-muted-foreground" : "text-destructive"}`}
          >
            {tests.ffprobe.ok ? "✓ " : "✗ "}
            {tests.ffprobe.message}
          </p>
        )}
      </Section>

      <Section title="After export">
        <p className="text-xs text-muted-foreground">
          What to do with the original recording after a clip exports successfully.
        </p>
        <Field label="Action">
          <Select
            value={ae.action}
            onValueChange={(v) => v && setAfter({ action: v as AfterExportAction })}
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="nothing">Do nothing</SelectItem>
              <SelectItem value="delete">Delete</SelectItem>
              <SelectItem value="move">Move to folder</SelectItem>
              <SelectItem value="rename">Rename (prefix/suffix)</SelectItem>
              <SelectItem value="prompt">Prompt each time</SelectItem>
            </SelectContent>
          </Select>
        </Field>
        {ae.action === "move" && (
          <div className="flex gap-2">
            <Input
              value={ae.moveFolder}
              placeholder="Destination folder"
              onChange={(e) => setAfter({ moveFolder: e.target.value })}
            />
            <Button variant="outline" onClick={pickMoveFolder}>
              Browse…
            </Button>
          </div>
        )}
        {ae.action === "rename" && (
          <div className="flex flex-wrap gap-3">
            <Field label="Prefix">
              <Input
                value={ae.renamePrefix}
                placeholder="e.g. done-"
                onChange={(e) => setAfter({ renamePrefix: e.target.value })}
              />
            </Field>
            <Field label="Suffix">
              <Input
                value={ae.renameSuffix}
                placeholder="e.g. -archived"
                onChange={(e) => setAfter({ renameSuffix: e.target.value })}
              />
            </Field>
          </div>
        )}
      </Section>

      <Section title="Preview cache">
        <p className="text-xs text-muted-foreground">
          Previews of clips the player can't open directly (MKV, HEVC) are cached here and reused.
          Exports always use the original file, so clearing this only frees disk space.
        </p>
        <div className="flex items-center gap-3">
          <Button variant="outline" size="sm" onClick={clearCache} disabled={clearing}>
            {clearing ? "Clearing…" : "Clear preview cache"}
          </Button>
          <span className="text-xs text-muted-foreground">
            {cacheBytes == null ? "…" : `${formatBytes(cacheBytes)} cached`}
          </span>
        </div>
      </Section>

      <div className="flex items-center gap-3">
        <Button variant="outline" size="sm" onClick={openConfigFile}>
          Open config file
        </Button>
        <span className="text-xs text-muted-foreground">Settings save automatically.</span>
      </div>
    </div>
  );
}
