import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import {
  type AppConfig,
  type AudioTrackSpec,
  buildRenamedFileName,
  type CropSpec,
  defaultEditSpec,
  type EditSpec,
  effectiveDuration,
  formatBytes,
  formatDuration,
  type MediaInfo,
  type OutputSettings,
  type QualityMode,
  type QualityPreset,
  type QueueItem,
  splitFileName,
  validateEditSpec,
} from "@qcksys/qlipq-core";
import {
  buildExportArgs,
  estimateExportSize,
  outputSettingsToEncode,
  parseFfprobe,
  parseProgress,
  progressFraction,
} from "@qcksys/qlipq-ffmpeg";
import * as api from "../lib/api.ts";
import { joinPath } from "../lib/queue.ts";
import { AudioPanel } from "./AudioPanel.tsx";
import { Timeline } from "./Timeline.tsx";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface EditorProps {
  item: QueueItem;
  config: AppConfig;
  onPatch: (id: string, patch: Partial<QueueItem>) => void;
  /** Audio enable/levels carried over from the previously edited clip. */
  audioDefaults: AudioTrackSpec[];
  onAudioDefaults: (tracks: AudioTrackSpec[]) => void;
}

function clampInt(value: string, min: number, max: number): number {
  const n = Math.round(Number(value));
  if (Number.isNaN(n)) return min;
  return Math.min(max, Math.max(min, n));
}

export function Editor({ item, config, onPatch, audioDefaults, onAudioDefaults }: EditorProps) {
  const [media, setMedia] = useState<MediaInfo | null>(null);
  const [spec, setSpec] = useState<EditSpec>({ audioTracks: [] });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [currentTime, setCurrentTime] = useState(0);
  const [exporting, setExporting] = useState(false);
  const [progress, setProgress] = useState(0);
  const [newTag, setNewTag] = useState("");
  const videoRef = useRef<HTMLVideoElement>(null);

  // Read via refs inside the probe effect so updating them doesn't re-probe.
  const itemEditRef = useRef(item.edit);
  itemEditRef.current = item.edit;
  const audioDefaultsRef = useRef(audioDefaults);
  audioDefaultsRef.current = audioDefaults;
  // Tracks which item the current spec belongs to, so autosave never writes a
  // stale spec to a just-switched clip.
  const specItemRef = useRef<string | null>(null);

  // Probe the clip whenever the selected clip changes (not on every edit).
  useEffect(() => {
    let cancelled = false;
    setMedia(null);
    setLoadError(null);
    setCurrentTime(0);
    api
      .probeRaw(item.path, config.ffprobePath)
      .then((raw) => {
        if (cancelled) return;
        const info = parseFfprobe(raw);
        setMedia(info);
        onPatch(item.id, { durationSec: info.durationSec });
        let next: EditSpec = itemEditRef.current ?? {
          ...defaultEditSpec(info),
          trim: { startSec: 0, endSec: info.durationSec },
        };
        // Carry audio enable/levels from the previous clip when this one is fresh.
        if (!itemEditRef.current && audioDefaultsRef.current.length > 0) {
          const defs = audioDefaultsRef.current;
          next = {
            ...next,
            audioTracks: next.audioTracks.map((track) => {
              const carried = defs.find((d) => d.index === track.index);
              return carried
                ? { ...track, enabled: carried.enabled, volume: carried.volume }
                : track;
            }),
          };
        }
        specItemRef.current = item.id;
        setSpec(next);
      })
      .catch((err: unknown) => {
        if (!cancelled) setLoadError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [item.id, item.path, config.ffprobePath, onPatch]);

  // Autosave the working edit to the queue item (debounced) so it survives tab
  // switches and app restarts. Guarded so a stale spec isn't written mid-switch.
  useEffect(() => {
    if (!media || specItemRef.current !== item.id) return;
    const id = setTimeout(() => onPatch(item.id, { edit: spec }), 300);
    return () => clearTimeout(id);
  }, [spec, media, item.id, onPatch]);

  // Effective output = global defaults with this clip's quality override applied.
  const output = useMemo<OutputSettings>(
    () => ({ ...config.output, ...item.outputOverride }),
    [config.output, item.outputOverride],
  );

  const trim = spec.trim ?? { startSec: 0, endSec: media?.durationSec ?? 0 };

  const validationError = useMemo(
    () => (media ? validateEditSpec(spec, media) : null),
    [spec, media],
  );

  const estimate = useMemo(
    () => (media ? estimateExportSize(media, spec, outputSettingsToEncode(output, media)) : null),
    [media, spec, output],
  );

  const setCrop = (crop: CropSpec | undefined) => setSpec((s) => ({ ...s, crop }));

  const toggleCrop = (enabled: boolean) => {
    if (!media) return;
    setCrop(enabled ? { x: 0, y: 0, width: media.width, height: media.height } : undefined);
  };

  const updateCrop = (patch: Partial<CropSpec>) => {
    if (!spec.crop) return;
    setCrop({ ...spec.crop, ...patch });
  };

  const tags = item.tags ?? [];
  const addTag = () => {
    const t = newTag.trim();
    if (t && !tags.includes(t)) onPatch(item.id, { tags: [...tags, t] });
    setNewTag("");
  };
  const removeTag = (t: string) => onPatch(item.id, { tags: tags.filter((x) => x !== t) });

  const setOverride = (patch: Partial<OutputSettings>) =>
    onPatch(item.id, { outputOverride: { ...item.outputOverride, ...patch } });

  const toggleOverride = (on: boolean) =>
    onPatch(item.id, {
      outputOverride: on
        ? {
            qualityMode: config.output.qualityMode,
            qualityPreset: config.output.qualityPreset,
            crf: config.output.crf,
            videoBitrateKbps: config.output.videoBitrateKbps,
          }
        : undefined,
    });

  const seek = (sec: number) => {
    setCurrentTime(sec);
    if (videoRef.current) videoRef.current.currentTime = sec;
  };

  const onExport = async () => {
    if (!media || validationError) return;
    const { name } = splitFileName(item.fileName);
    // Output container is chosen in settings, so override the source extension.
    const outName = buildExportName(config, item, name, output.container);
    const outputPath = joinPath(config.outputFolder, outName);
    const { video, audio, reencode } = outputSettingsToEncode(output, media);
    const args = buildExportArgs({
      inputPath: item.path,
      outputPath,
      spec,
      progress: true,
      video,
      audio,
      reencode,
      // Stamp the inferred game (filename prefix or NVIDIA per-game folder) into the clip.
      metadata: item.source ? { game: item.source } : undefined,
    });

    setExporting(true);
    setProgress(0);
    onPatch(item.id, { status: "exporting", edit: spec, error: undefined });

    const total = effectiveDuration(spec, media);
    const unlisten = await api.onExportProgress((event) => {
      if (event.id !== item.id) return;
      const { outTimeSec } = parseProgress(event.line);
      setProgress(progressFraction(outTimeSec, total));
    });

    try {
      await api.runExport(item.id, config.ffmpegPath, args);
      setProgress(1);
      onPatch(item.id, { status: "done", exportPath: outputPath });
      if (config.deleteSourceAfterExport) await api.deleteFile(item.path);
    } catch (err) {
      onPatch(item.id, { status: "error", error: String(err) });
    } finally {
      unlisten();
      setExporting(false);
    }
  };

  if (loadError) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
        <p className="text-sm font-medium text-destructive">Could not read this clip.</p>
        <pre className="max-w-full overflow-auto rounded-lg bg-muted p-3 text-left text-xs text-muted-foreground">
          {loadError}
        </pre>
      </div>
    );
  }

  if (!media) {
    return (
      <div className="flex h-full items-center justify-center p-6">
        <p className="text-sm text-muted-foreground">Reading clip…</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4 p-4">
      <div className="overflow-hidden rounded-xl border border-border bg-black">
        {/* eslint-disable-next-line jsx-a11y/media-has-caption */}
        <video
          ref={videoRef}
          className="max-h-[48vh] w-full"
          src={api.fileUrl(item.path)}
          controls
          onTimeUpdate={(e) => setCurrentTime(e.currentTarget.currentTime)}
        />
      </div>

      <Timeline
        duration={media.durationSec}
        trim={trim}
        currentTime={currentTime}
        onChange={(t) => setSpec((s) => ({ ...s, trim: t }))}
        onSeek={seek}
      />

      <div className="grid gap-4 md:grid-cols-2">
        <section className="flex flex-col gap-3 rounded-xl border border-border bg-card p-4">
          <h3 className="text-sm font-semibold">Crop</h3>
          <label className="flex items-center gap-2 text-sm">
            <Checkbox
              checked={!!spec.crop}
              onCheckedChange={(checked) => toggleCrop(checked === true)}
            />
            Enable crop ({media.width}×{media.height} source)
          </label>
          {spec.crop && (
            <div className="grid grid-cols-2 gap-3">
              <NumberField
                label="X"
                value={spec.crop.x}
                max={media.width}
                onChange={(x) => updateCrop({ x })}
              />
              <NumberField
                label="Y"
                value={spec.crop.y}
                max={media.height}
                onChange={(y) => updateCrop({ y })}
              />
              <NumberField
                label="Width"
                value={spec.crop.width}
                max={media.width}
                onChange={(width) => updateCrop({ width })}
              />
              <NumberField
                label="Height"
                value={spec.crop.height}
                max={media.height}
                onChange={(height) => updateCrop({ height })}
              />
            </div>
          )}
        </section>

        <section className="flex flex-col gap-3 rounded-xl border border-border bg-card p-4">
          <h3 className="text-sm font-semibold">Audio tracks</h3>
          <AudioPanel
            streams={media.audioStreams}
            tracks={spec.audioTracks}
            onChange={(audioTracks) => {
              setSpec((s) => ({ ...s, audioTracks }));
              onAudioDefaults(audioTracks);
            }}
          />
        </section>
      </div>

      <section className="flex flex-col gap-3 rounded-xl border border-border bg-card p-4">
        <label className="flex items-center gap-2 text-sm font-semibold">
          <Checkbox
            checked={!!item.outputOverride}
            onCheckedChange={(checked) => toggleOverride(checked === true)}
          />
          Override quality for this clip
        </label>
        {item.outputOverride && (
          <div className="flex flex-wrap gap-3">
            <OverrideField label="Quality">
              <Select
                value={output.qualityMode}
                onValueChange={(v) => v && setOverride({ qualityMode: v as QualityMode })}
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
            </OverrideField>
            {output.qualityMode === "preset" && (
              <OverrideField label="Preset">
                <Select
                  value={output.qualityPreset}
                  onValueChange={(v) => v && setOverride({ qualityPreset: v as QualityPreset })}
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
              </OverrideField>
            )}
            {(output.qualityMode === "crf" || output.qualityMode === "vbr") && (
              <OverrideField label="CRF (0–51)">
                <Input
                  type="number"
                  min={0}
                  max={51}
                  value={output.crf}
                  onChange={(e) => setOverride({ crf: clampInt(e.target.value, 0, 51) })}
                />
              </OverrideField>
            )}
            {(output.qualityMode === "bitrate" || output.qualityMode === "vbr") && (
              <OverrideField
                label={output.qualityMode === "vbr" ? "Max bitrate (kbps)" : "Video bitrate (kbps)"}
              >
                <Input
                  type="number"
                  min={100}
                  step={500}
                  value={output.videoBitrateKbps}
                  onChange={(e) =>
                    setOverride({ videoBitrateKbps: clampInt(e.target.value, 100, 200000) })
                  }
                />
              </OverrideField>
            )}
          </div>
        )}
      </section>

      <section className="flex flex-col gap-2 rounded-xl border border-border bg-card p-4">
        <h3 className="text-sm font-semibold">Tags</h3>
        <div className="flex flex-wrap items-center gap-1.5">
          {tags.map((tag) => (
            <Badge key={tag} variant="secondary" className="gap-1">
              {tag}
              <button
                type="button"
                className="text-muted-foreground hover:text-foreground"
                aria-label={`Remove ${tag}`}
                onClick={() => removeTag(tag)}
              >
                ×
              </button>
            </Badge>
          ))}
          <input
            className="h-7 min-w-32 flex-1 rounded-md border border-input bg-transparent px-2 text-sm outline-none focus-visible:border-ring"
            placeholder="Add tag…"
            value={newTag}
            onChange={(e) => setNewTag(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addTag();
            }}
          />
        </div>
      </section>

      <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border bg-card p-4">
        <div className="text-sm text-muted-foreground">
          <strong className="text-foreground">
            {formatDuration(effectiveDuration(spec, media))}
          </strong>{" "}
          output ·{" "}
          {spec.crop ? `${spec.crop.width}×${spec.crop.height}` : `${media.width}×${media.height}`}
          {estimate && (
            <>
              {" · "}
              {estimate.approximate ? "≈" : ""}
              {formatBytes(estimate.bytes)}
            </>
          )}
        </div>
        {validationError && <span className="text-sm text-destructive">{validationError}</span>}
        {exporting && <Progress className="min-w-40 flex-1" value={Math.round(progress * 100)} />}
        <Button
          className="ml-auto"
          disabled={exporting || !!validationError || !config.outputFolder}
          onClick={onExport}
          title={!config.outputFolder ? "Set an output folder in Settings first" : undefined}
        >
          {exporting ? `Exporting ${Math.round(progress * 100)}%` : "Export clip"}
        </Button>
      </div>
    </div>
  );
}

// Reuse rename templating so exports are named consistently with renames.
function buildExportName(config: AppConfig, item: QueueItem, name: string, ext: string): string {
  const recordedAt = item.recordedAt ? new Date(item.recordedAt) : undefined;
  return buildRenamedFileName(config.namingTemplate, {
    name,
    ext,
    recordedAt,
    source: item.source,
  });
}

function OverrideField({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-1 flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </div>
  );
}

interface NumberFieldProps {
  label: string;
  value: number;
  max: number;
  onChange: (value: number) => void;
}

function NumberField({ label, value, max, onChange }: NumberFieldProps) {
  return (
    <label className="flex flex-col gap-1 text-xs text-muted-foreground">
      {label}
      <Input
        type="number"
        min={0}
        max={max}
        value={value}
        onChange={(e) => onChange(Math.round(Number(e.target.value)))}
      />
    </label>
  );
}
