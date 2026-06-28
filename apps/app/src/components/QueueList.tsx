import { formatDate, formatTime, type QueueItem, type QueueStatus } from "@qlipq/core";

interface QueueListProps {
  items: QueueItem[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onRename: (item: QueueItem) => void;
  onRemove: (id: string) => void;
}

const STATUS_LABEL: Record<QueueStatus, string> = {
  pending: "Pending",
  ready: "Ready",
  editing: "Editing",
  exporting: "Exporting",
  done: "Done",
  error: "Error",
};

export function QueueList({ items, selectedId, onSelect, onRename, onRemove }: QueueListProps) {
  if (items.length === 0) {
    return (
      <p className="muted queue-empty">Queue is empty. Add a watched folder to populate it.</p>
    );
  }

  return (
    <ul className="queue">
      {items.map((item) => {
        const recordedAt = item.recordedAt ? new Date(item.recordedAt) : null;
        return (
          <li
            key={item.id}
            className={`queue-item ${item.id === selectedId ? "selected" : ""}`}
            onClick={() => onSelect(item.id)}
          >
            <div className="queue-main">
              <span className="queue-name" title={item.path}>
                {item.fileName}
              </span>
              <span className="queue-meta muted small">
                {item.source ? `${item.source} · ` : ""}
                {recordedAt
                  ? `${formatDate(recordedAt)} ${formatTime(recordedAt)}`
                  : "Unknown time"}
              </span>
            </div>
            <span className={`badge status-${item.status}`}>{STATUS_LABEL[item.status]}</span>
            <div className="queue-actions">
              <button
                type="button"
                className="link"
                onClick={(e) => {
                  e.stopPropagation();
                  onRename(item);
                }}
              >
                Rename
              </button>
              <button
                type="button"
                className="link"
                onClick={(e) => {
                  e.stopPropagation();
                  onRemove(item.id);
                }}
              >
                Remove
              </button>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
