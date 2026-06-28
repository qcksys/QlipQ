import { type AudioStreamInfo, audioStreamLabel, type AudioTrackSpec } from "@qlipq/core";

interface AudioPanelProps {
  streams: AudioStreamInfo[];
  tracks: AudioTrackSpec[];
  onChange: (tracks: AudioTrackSpec[]) => void;
}

/** Per-track enable toggle and volume slider (linear gain, shown as a percentage). */
export function AudioPanel({ streams, tracks, onChange }: AudioPanelProps) {
  if (streams.length === 0) {
    return <p className="muted">No audio tracks in this clip.</p>;
  }

  const update = (index: number, patch: Partial<AudioTrackSpec>) => {
    onChange(tracks.map((track) => (track.index === index ? { ...track, ...patch } : track)));
  };

  return (
    <ul className="audio-list">
      {streams.map((stream) => {
        const track = tracks.find((t) => t.index === stream.index);
        if (!track) return null;
        return (
          <li key={stream.index} className="audio-track">
            <label className="audio-enable">
              <input
                type="checkbox"
                checked={track.enabled}
                onChange={(e) => update(stream.index, { enabled: e.target.checked })}
              />
              <span>{audioStreamLabel(stream)}</span>
              <span className="muted small">
                {stream.codec} · {stream.channels}ch
              </span>
            </label>
            <div className="audio-volume">
              <input
                type="range"
                min={0}
                max={2}
                step={0.05}
                value={track.volume}
                disabled={!track.enabled}
                onChange={(e) => update(stream.index, { volume: Number(e.target.value) })}
                aria-label={`${audioStreamLabel(stream)} volume`}
              />
              <span className="volume-value">{Math.round(track.volume * 100)}%</span>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
