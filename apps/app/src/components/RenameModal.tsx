import { useState } from "react";
import { buildRenamedFileName, type QueueItem, splitFileName } from "@qlipq/core";

interface RenameModalProps {
  item: QueueItem;
  namingTemplate: string;
  onCancel: () => void;
  onConfirm: (newFileName: string) => void;
}

/** Rename a queued recording in place, with a one-click suggestion from the template. */
export function RenameModal({ item, namingTemplate, onCancel, onConfirm }: RenameModalProps) {
  const { name, ext } = splitFileName(item.fileName);
  const [value, setValue] = useState(name);

  const suggest = () => {
    const recordedAt = item.recordedAt ? new Date(item.recordedAt) : undefined;
    const suggested = buildRenamedFileName(namingTemplate, {
      name,
      ext,
      recordedAt,
      source: item.source,
    });
    setValue(splitFileName(suggested).name);
  };

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed) return;
    onConfirm(ext ? `${trimmed}.${ext}` : trimmed);
  };

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h3>Rename recording</h3>
        <p className="muted small">{item.fileName}</p>
        <div className="row">
          <input
            type="text"
            value={value}
            autoFocus
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submit();
              if (e.key === "Escape") onCancel();
            }}
          />
          <span className="ext-suffix">.{ext}</span>
        </div>
        <div className="modal-actions">
          <button type="button" className="link" onClick={suggest}>
            Use template
          </button>
          <span className="spacer" />
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="primary" onClick={submit}>
            Rename
          </button>
        </div>
      </div>
    </div>
  );
}
