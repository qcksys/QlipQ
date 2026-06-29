import { useEffect, useMemo, useRef, useState } from "react";
import {
  type AppConfig,
  buildRenamedFileName,
  type CropSpec,
  defaultEditSpec,
  type EditSpec,
  effectiveDuration,
  formatBytes,
  formatDuration,
  type MediaInfo,
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
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";

interface EditorProps {
  item: QueueItem;
  config: AppConfig;
  onPatch: (id: string, patch: Partial<QueueItem>) => void;
}

export function Editor({ item, config, onPatch }: EditorProps) {
  const [media, setMedia] = useState<MediaInfo | null>(null);
  const [spec, setSpec] = useState<EditSpec>({ audioTracks: [] });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [currentTime, setCurrentTime] = useState(0);
  const [exporting, setExporting] = useState(false);
  const [progress, setProgress] = useState(0);
  const videoRef = useRef<HTMLVideoElement>(null);

  // Probe the clip whenever the selected item changes.
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
        setSpec(
          item.edit ?? {
            ...defaultEditSpec(info),
            trim: { startSec: 0, endSec: info.durationSec },
          },
        );
      })
      .catch((err: unknown) => {
        if (!cancelled) setLoadError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [item.id, item.path, item.edit, config.ffprobePath]);

  const trim = spec.trim ?? { startSec: 0, endSec: media?.durationSec ?? 0 };

  const validationError = useMemo(
    () => (media ? validateEditSpec(spec, media) : null),
    [spec, media],
  );

  const estimate = useMemo(
    () =>
      media ? estimateExportSize(media, spec, outputSettingsToEncode(config.output, media)) : null,
    [media, spec, config.output],
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

  const seek = (sec: number) => {
    setCurrentTime(sec);
    if (videoRef.current) videoRef.current.currentTime = sec;
  };

  const onExport = async () => {
    if (!media || validationError) return;
    const { name } = splitFileName(item.fileName);
    // Output container is chosen in settings, so override the source extension.
    const outName = buildExportName(config, item, name, config.output.container);
    const outputPath = joinPath(config.outputFolder, outName);
    const { video, audio, reencode } = outputSettingsToEncode(config.output, media);
    const args = buildExportArgs({
      inputPath: item.path,
      outputPath,
      spec,
      progress: true,
      video,
      audio,
      reencode,
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
            onChange={(audioTracks) => setSpec((s) => ({ ...s, audioTracks }))}
          />
        </section>
      </div>

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
