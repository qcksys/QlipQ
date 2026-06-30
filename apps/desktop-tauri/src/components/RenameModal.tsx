import { useState } from "react";
import { buildRenamedFileName, type QueueItem, splitFileName } from "@qcksys/qlipq-core";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

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
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Rename recording</DialogTitle>
          <DialogDescription className="truncate">{item.fileName}</DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-2">
          <Input
            autoFocus
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submit();
              if (e.key === "Escape") onCancel();
            }}
          />
          {ext && <span className="text-sm whitespace-nowrap text-muted-foreground">.{ext}</span>}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={suggest} className="sm:mr-auto">
            Use template
          </Button>
          <Button variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button onClick={submit}>Rename</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
