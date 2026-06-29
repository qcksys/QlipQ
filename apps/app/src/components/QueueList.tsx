import { formatDate, formatTime, type QueueItem, type QueueStatus } from "@qcksys/qlipq-core";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

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

const STATUS_VARIANT: Record<QueueStatus, "default" | "secondary" | "destructive" | "outline"> = {
  pending: "outline",
  ready: "secondary",
  editing: "secondary",
  exporting: "secondary",
  done: "default",
  error: "destructive",
};

export function QueueList({ items, selectedId, onSelect, onRename, onRemove }: QueueListProps) {
  if (items.length === 0) {
    return (
      <p className="p-4 text-sm text-muted-foreground">
        Queue is empty. Add a watched folder to populate it.
      </p>
    );
  }

  return (
    <ul className="flex flex-col gap-1.5">
      {items.map((item) => {
        const recordedAt = item.recordedAt ? new Date(item.recordedAt) : null;
        return (
          <li
            key={item.id}
            onClick={() => onSelect(item.id)}
            className={cn(
              "cursor-pointer rounded-lg border border-border bg-card p-3 transition-colors hover:border-muted-foreground/40",
              item.id === selectedId && "border-primary bg-accent",
            )}
          >
            <div className="flex items-start justify-between gap-2">
              <div className="min-w-0">
                <div className="truncate text-sm font-semibold" title={item.path}>
                  {item.fileName}
                </div>
                <div className="text-xs text-muted-foreground">
                  {item.source ? `${item.source} · ` : ""}
                  {recordedAt
                    ? `${formatDate(recordedAt)} ${formatTime(recordedAt)}`
                    : "Unknown time"}
                </div>
              </div>
              <Badge variant={STATUS_VARIANT[item.status]}>{STATUS_LABEL[item.status]}</Badge>
            </div>
            <div className="mt-2 flex gap-3">
              <Button
                variant="link"
                size="sm"
                className="h-auto p-0"
                onClick={(e) => {
                  e.stopPropagation();
                  onRename(item);
                }}
              >
                Rename
              </Button>
              <Button
                variant="link"
                size="sm"
                className="h-auto p-0"
                onClick={(e) => {
                  e.stopPropagation();
                  onRemove(item.id);
                }}
              >
                Remove
              </Button>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
