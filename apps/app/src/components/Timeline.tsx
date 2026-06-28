import { formatDuration, type TrimSpec } from "@qlipq/core";

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
    <div className="timeline">
      <div className="timeline-track">
        <div
          className="timeline-selection"
          style={{ left: `${startPct}%`, right: `${100 - endPct}%` }}
        />
        <div className="timeline-playhead" style={{ left: `${playPct}%` }} />
        <input
          className="timeline-scrub"
          type="range"
          min={0}
          max={duration || 0}
          step={0.01}
          value={currentTime}
          onChange={(e) => onSeek(Number(e.target.value))}
          aria-label="Scrub"
        />
      </div>

      <div className="trim-row">
        <label>
          In
          <input
            type="number"
            min={0}
            max={duration}
            step={0.1}
            value={Number(trim.startSec.toFixed(2))}
            onChange={(e) => setStart(Number(e.target.value))}
          />
        </label>
        <button type="button" onClick={() => setStart(currentTime)}>
          Set in at playhead
        </button>
        <span className="trim-length">{formatDuration(trim.endSec - trim.startSec)}</span>
        <button type="button" onClick={() => setEnd(currentTime)}>
          Set out at playhead
        </button>
        <label>
          Out
          <input
            type="number"
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
