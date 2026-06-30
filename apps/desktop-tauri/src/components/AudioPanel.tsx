import { type AudioStreamInfo, audioStreamLabel, type AudioTrackSpec } from "@qcksys/qlipq-core";
import { Checkbox } from "@/components/ui/checkbox";
import { Slider } from "@/components/ui/slider";

interface AudioPanelProps {
  streams: AudioStreamInfo[];
  tracks: AudioTrackSpec[];
  onChange: (tracks: AudioTrackSpec[]) => void;
}

/** Per-track enable toggle and volume slider (linear gain, shown as a percentage). */
export function AudioPanel({ streams, tracks, onChange }: AudioPanelProps) {
  if (streams.length === 0) {
    return <p className="text-sm text-muted-foreground">No audio tracks in this clip.</p>;
  }

  const update = (index: number, patch: Partial<AudioTrackSpec>) => {
    onChange(tracks.map((track) => (track.index === index ? { ...track, ...patch } : track)));
  };

  // A single enabled track plays through the <video> element, which caps volume at 100%.
  // (Multi-track boosts are baked into the preview proxy, so they're audible there.)
  const enabledTracks = tracks.filter((t) => t.enabled);
  const previewCapped = enabledTracks.length === 1 && enabledTracks[0].volume > 1;

  return (
    <div className="flex flex-col gap-2">
      <ul className="flex flex-col gap-3">
        {streams.map((stream) => {
          const track = tracks.find((t) => t.index === stream.index);
          if (!track) return null;
          const label = audioStreamLabel(stream);
          return (
            <li key={stream.index} className="flex flex-col gap-2">
              <label className="flex items-center gap-2">
                <Checkbox
                  checked={track.enabled}
                  onCheckedChange={(checked) => update(stream.index, { enabled: checked === true })}
                />
                <span className="text-sm font-medium">{label}</span>
                <span className="text-xs text-muted-foreground">
                  {stream.codec} · {stream.channels}ch
                </span>
              </label>
              <div className="flex items-center gap-3 pl-6">
                <Slider
                  className="flex-1"
                  min={0}
                  max={2}
                  step={0.05}
                  value={[track.volume]}
                  disabled={!track.enabled}
                  onValueChange={(value) =>
                    update(stream.index, { volume: Array.isArray(value) ? value[0] : value })
                  }
                  aria-label={`${label} volume`}
                />
                <span className="w-10 text-right text-xs text-muted-foreground tabular-nums">
                  {Math.round(track.volume * 100)}%
                </span>
              </div>
            </li>
          );
        })}
      </ul>
      {previewCapped && (
        <p className="text-xs text-muted-foreground">
          Preview plays this track at 100%; the export still applies the full boost.
        </p>
      )}
    </div>
  );
}
