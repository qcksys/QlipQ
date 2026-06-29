import { formatDuration, type TrimSpec } from "@qcksys/qlipq-core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

interface TimelineProps {
  duration: number;
  trim: TrimSpec;
  currentTime: number;
  onChange: (trim: TrimSpec) => void;
  onSeek: (sec: number) => void;
}

/** Trim controls: a scrubber plus in/out handles, with set-at-playhead helpers. */
export function Timeline({ duration, trim, currentTime, onChange, onSeek }: TimelineProps) {
  const clamp = (value: number) => Math.min(duration, Math.max(0, value));

  const setStart = (value: number) => {
    const startSec = clamp(Math.min(value, trim.endSec - 0.1));
    onChange({ ...trim, startSec });
  };
  const setEnd = (value: number) => {
    const endSec = clamp(Math.max(value, trim.startSec + 0.1));
    onChange({ ...trim, endSec });
  };

  const startPct = duration > 0 ? (trim.startSec / duration) * 100 : 0;
  const endPct = duration > 0 ? (trim.endSec / duration) * 100 : 100;
  const playPct = duration > 0 ? (currentTime / duration) * 100 : 0;

  return (
    <div className="flex flex-col gap-3">
      <div className="relative h-6">
        <div className="absolute inset-x-0 top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-muted" />
        <div
          className="absolute top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-primary/50"
          style={{ left: `${startPct}%`, right: `${100 - endPct}%` }}
        />
        <div
          className="absolute top-0 h-full w-0.5 bg-foreground"
          style={{ left: `${playPct}%` }}
        />
        <input
          className="absolute inset-0 w-full cursor-pointer appearance-none bg-transparent"
          type="range"
          min={0}
          max={duration || 0}
          step={0.01}
          value={currentTime}
          onChange={(e) => onSeek(Number(e.target.value))}
          aria-label="Scrub"
        />
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
          In
          <Input
            type="number"
            className="w-24"
            min={0}
            max={duration}
            step={0.1}
            value={Number(trim.startSec.toFixed(2))}
            onChange={(e) => setStart(Number(e.target.value))}
          />
        </label>
        <Button variant="outline" size="sm" onClick={() => setStart(currentTime)}>
          Set in at playhead
        </Button>
        <span className="text-sm font-medium tabular-nums">
          {formatDuration(trim.endSec - trim.startSec)}
        </span>
        <Button variant="outline" size="sm" onClick={() => setEnd(currentTime)}>
          Set out at playhead
        </Button>
        <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
          Out
          <Input
            type="number"
            className="w-24"
            min={0}
            max={duration}
            step={0.1}
            value={Number(trim.endSec.toFixed(2))}
            onChange={(e) => setEnd(Number(e.target.value))}
          />
        </label>
      </div>
    </div>
  );
}
