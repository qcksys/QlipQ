/** Metadata recovered from an OBS recording/replay-buffer filename. */
export interface ParsedRecording {
  /** Local timestamp parsed from the filename, if present. */
  recordedAt?: Date;
  /** A leading label (OBS scene/profile or game name) before the timestamp, if present. */
  source?: string;
  /** Whether the filename looks like a replay-buffer clip ("Replay ..."). */
  isReplay: boolean;
}

// Matches the date/time portion OBS writes with its default and common custom
// formats, e.g. "2024-01-31 18-09-05", "2024-01-31_18-09-05",
// "2024-01-31 18.09.05". Capture groups: y m d H M S.
const TIMESTAMP = /(\d{4})-(\d{2})-(\d{2})[ _T-](\d{2})[-.:](\d{2})[-.:](\d{2})/;

/**
 * Parse an OBS recording filename into a timestamp and optional source label.
 *
 * OBS filenames are driven by the user's "Filename Formatting" setting; the
 * default is `%CCYY-%MM-%DD %hh-%mm-%ss`, and the replay buffer prefixes
 * `Replay `. Many users prepend a scene or game name. We extract whatever
 * timestamp we can find and treat text before it as the source label.
 */
export function parseObsFilename(fileName: string): ParsedRecording {
  const base = stripExtension(fileName);
  const match = TIMESTAMP.exec(base);

  let isReplay = /\breplay\b/i.test(base);

  if (!match) {
    return { isReplay };
  }

  const [full, y, mo, d, h, mi, s] = match;
  const recordedAt = new Date(
    Number(y),
    Number(mo) - 1,
    Number(d),
    Number(h),
    Number(mi),
    Number(s),
  );

  const lead = base.slice(0, match.index).trim();
  const cleanedLead = lead.replace(/[_\-.]+$/, "").trim();
  let source: string | undefined;
  if (cleanedLead && !/^replay$/i.test(cleanedLead)) {
    source = cleanedLead.replace(/^replay[_\-. ]+/i, "").trim() || undefined;
  }
  if (/^replay\b/i.test(lead)) isReplay = true;

  // `full` is referenced to keep the destructure explicit; not otherwise used.
  void full;

  return { recordedAt, source, isReplay };
}

function stripExtension(fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  return dot > 0 ? fileName.slice(0, dot) : fileName;
}
