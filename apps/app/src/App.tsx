import { useCallback, useEffect, useRef, useState } from "react";
import {
  type AppConfig,
  type AudioTrackSpec,
  DEFAULT_CONFIG,
  type EditSpec,
  type OutputSettings,
  parseObsFilename,
  type QueueItem,
} from "@qcksys/qlipq-core";
import { parseFfprobe } from "@qcksys/qlipq-ffmpeg";
import { toast } from "sonner";
import { ConfigPanel } from "./components/ConfigPanel.tsx";
import { Editor } from "./components/Editor.tsx";
import { QueueList } from "./components/QueueList.tsx";
import { RenameModal } from "./components/RenameModal.tsx";
import * as api from "./lib/api.ts";
import { basename, dirname, joinPath, queueItemFromPath, toPosixPath } from "./lib/queue.ts";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";

type View = "queue" | "settings";

// Remembered after the first confirmed delete-from-disk so we don't keep asking.
const DELETE_CONFIRMED_KEY = "qlipq.deleteConfirmed";

// Per-file edit state persisted to disk (keyed by path) so trims, per-clip quality
// overrides, and tags survive restarts.
const EDITS_FILE = "edits.json";
interface StoredEdit {
  edit?: EditSpec;
  outputOverride?: Partial<OutputSettings>;
  tags?: string[];
}

/** Human-readable summary of a (re)scan: how many clips were newly queued. */
function describeScan(added: number, scanned: number, folder?: string): string {
  const where = folder ? ` in ${folder}` : "";
  if (added > 0) return `Added ${added} new clip${added === 1 ? "" : "s"}${where}.`;
  if (scanned === 0) return `No video files found${where}.`;
  return `No new clips${where} — all ${scanned} already in the queue.`;
}

export function App() {
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [items, setItems] = useState<QueueItem[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("queue");
  const [renameTarget, setRenameTarget] = useState<QueueItem | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<QueueItem | null>(null);
  const [ready, setReady] = useState(false);
  const [presets, setPresets] = useState<api.CapturePresets>({});
  const [audioDefaults, setAudioDefaults] = useState<AudioTrackSpec[]>([]);
  const [tagFilter, setTagFilter] = useState<string | null>(null);

  // Avoid duplicate queue entries for the same path (scan + watcher overlap).
  const knownPaths = useRef(new Set<string>());
  // Mirrors of state for use inside stable callbacks without stale closures.
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const configRef = useRef(config);
  configRef.current = config;
  // Disk-backed per-file edit store (path -> edit/override/tags).
  const editStore = useRef<Record<string, StoredEdit>>({});
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const scheduleSaveStore = useCallback(() => {
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      api
        .writeAppFile(EDITS_FILE, JSON.stringify(editStore.current))
        .catch((err: unknown) => console.error("save edits failed", err));
    }, 500);
  }, []);

  const persistEntry = useCallback(
    (path: string, patch: Partial<QueueItem>) => {
      const next: StoredEdit = { ...editStore.current[path] };
      if ("edit" in patch) next.edit = patch.edit;
      if ("outputOverride" in patch) next.outputOverride = patch.outputOverride;
      if ("tags" in patch) next.tags = patch.tags;
      editStore.current[path] = next;
      scheduleSaveStore();
    },
    [scheduleSaveStore],
  );

  const patchByPath = useCallback((path: string, patch: Partial<QueueItem>) => {
    setItems((current) =>
      current.map((item) => (item.path === path ? { ...item, ...patch } : item)),
    );
  }, []);

  // Fetch filesystem size + modified time for newly-added files (cheap, batched).
  const hydrateFileInfo = useCallback(async (paths: string[]) => {
    if (paths.length === 0) return;
    try {
      const infos = await api.fileInfo(paths);
      const byPath = new Map(infos.map((info) => [info.path, info]));
      setItems((current) =>
        current.map((item) => {
          const info = byPath.get(item.path);
          if (!info) return item;
          return {
            ...item,
            fileSizeBytes: info.size,
            fileModifiedAt: new Date(info.modifiedMs).toISOString(),
          };
        }),
      );
    } catch (err) {
      console.error("file info failed", err);
    }
  }, []);

  // Background duration probing, throttled so a large queue doesn't spawn hundreds of
  // ffprobe processes at once. Skips items already probed (incl. by the editor) or removed.
  const probeQueue = useRef<string[]>([]);
  const probeActive = useRef(0);
  const PROBE_CONCURRENCY = 3;
  const pumpProbe = useCallback(() => {
    while (probeActive.current < PROBE_CONCURRENCY && probeQueue.current.length > 0) {
      const path = probeQueue.current.shift();
      if (!path) break;
      const existing = itemsRef.current.find((item) => item.path === path);
      if (!existing || existing.durationSec != null) continue;
      probeActive.current += 1;
      api
        .probeRaw(path, configRef.current.ffprobePath)
        .then((raw) => patchByPath(path, { durationSec: parseFfprobe(raw).durationSec }))
        .catch(() => {
          /* leave duration unknown */
        })
        .finally(() => {
          probeActive.current -= 1;
          pumpProbe();
        });
    }
  }, [patchByPath]);
  const enqueueProbe = useCallback(
    (paths: string[]) => {
      probeQueue.current.push(...paths);
      pumpProbe();
    },
    [pumpProbe],
  );

  // Dedup and ref-mutation happen here (outside setItems) so the result is correct
  // under StrictMode, which invokes state updaters twice. Returns how many were added.
  const addPaths = useCallback(
    (paths: string[]): number => {
      const fresh: string[] = [];
      for (const raw of paths) {
        const path = toPosixPath(raw);
        if (!knownPaths.current.has(path)) {
          knownPaths.current.add(path);
          fresh.push(path);
        }
      }
      if (fresh.length === 0) return 0;
      const roots = configRef.current.watchedFolders;
      const additions = fresh.map((path) => {
        const base = queueItemFromPath(path, new Date().toISOString(), roots);
        const stored = editStore.current[path];
        return stored ? { ...base, ...stored } : base;
      });
      setItems((current) => [...additions, ...current]);
      void hydrateFileInfo(fresh);
      enqueueProbe(fresh);
      return fresh.length;
    },
    [hydrateFileInfo, enqueueProbe],
  );

  const loadFromFolders = useCallback(
    async (folders: string[], extensions: string[]) => {
      const found = await api.scanFolders(folders, extensions);
      addPaths(found);
      await api.startWatching(folders, extensions);
    },
    [addPaths],
  );

  // Initial load: config + watcher subscription. Scanning and persistence are handled
  // by the effects below so they also react to later config changes (auto-save).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      setConfig(await api.getConfig());
      // Load persisted per-file edits before the first scan so items hydrate from them.
      const storeText = await api.readAppFile(EDITS_FILE).catch(() => null);
      if (storeText) {
        try {
          editStore.current = JSON.parse(storeText);
        } catch (err) {
          console.error("edits.json parse failed", err);
        }
      }
      unlisten = await api.onFileAdded((path) => addPaths([path]));
      setReady(true);
      // Best-effort; detectCapturePresets resolves to {} if a source is unavailable.
      api
        .detectCapturePresets()
        .then(setPresets, (err: unknown) => console.error("preset detection failed", err));
    })().catch((err: unknown) => console.error("startup failed", err));
    return () => unlisten?.();
  }, [addPaths]);

  // Auto-save: persist config (debounced) whenever it changes, once loaded.
  useEffect(() => {
    if (!ready) return;
    const id = setTimeout(() => {
      api.setConfig(config).catch((err: unknown) => console.error("save config failed", err));
    }, 400);
    return () => clearTimeout(id);
  }, [config, ready]);

  // (Re)scan and watch whenever the watched folders or video extensions change.
  useEffect(() => {
    if (!ready) return;
    loadFromFolders(config.watchedFolders, config.videoExtensions).catch((err: unknown) =>
      console.error("scan failed", err),
    );
  }, [ready, config.watchedFolders, config.videoExtensions, loadFromFolders]);

  const patchConfig = (patch: Partial<AppConfig>) => setConfig((c) => ({ ...c, ...patch }));

  // Re-scan one or all watched folders and add any files not already queued.
  // Non-destructive: existing items keep their edits/status; files removed earlier
  // reappear (removeItem clears them from knownPaths).
  const reprocessFolder = useCallback(
    async (folder: string) => {
      const found = await api.scanFolders([folder], config.videoExtensions);
      toast(describeScan(addPaths(found), found.length, folder));
      setView("queue");
    },
    [addPaths, config.videoExtensions],
  );

  const rescanAllFolders = useCallback(async () => {
    const found = await api.scanFolders(config.watchedFolders, config.videoExtensions);
    toast(describeScan(addPaths(found), found.length));
  }, [addPaths, config.watchedFolders, config.videoExtensions]);

  const patchItem = useCallback(
    (id: string, patch: Partial<QueueItem>) => {
      if ("edit" in patch || "outputOverride" in patch || "tags" in patch) {
        const target = itemsRef.current.find((i) => i.id === id);
        if (target) persistEntry(target.path, patch);
      }
      setItems((current) => current.map((item) => (item.id === id ? { ...item, ...patch } : item)));
    },
    [persistEntry],
  );

  const removeItem = (id: string) => {
    setItems((current) => {
      const target = current.find((item) => item.id === id);
      if (target) knownPaths.current.delete(target.path);
      return current.filter((item) => item.id !== id);
    });
    if (selectedId === id) setSelectedId(null);
  };

  // Delete a clip from disk, then drop it from the queue. Confirmed once, then remembered.
  const performDelete = async (item: QueueItem) => {
    try {
      await api.deleteFile(item.path);
      removeItem(item.id);
    } catch (err) {
      toast.error(`Couldn't delete ${item.fileName}`);
      console.error("delete failed", err);
    }
  };

  const requestDelete = (item: QueueItem) => {
    if (localStorage.getItem(DELETE_CONFIRMED_KEY) === "1") {
      void performDelete(item);
    } else {
      setDeleteTarget(item);
    }
  };

  const confirmDelete = () => {
    if (!deleteTarget) return;
    localStorage.setItem(DELETE_CONFIRMED_KEY, "1");
    void performDelete(deleteTarget);
    setDeleteTarget(null);
  };

  const confirmRename = async (newFileName: string) => {
    const target = renameTarget;
    if (!target) return;
    const newPath = joinPath(dirname(target.path), newFileName);
    try {
      const finalPath = await api.renameFile(target.path, newPath);
      const finalName = basename(finalPath);
      const parsed = parseObsFilename(finalName);
      knownPaths.current.delete(target.path);
      knownPaths.current.add(finalPath);
      patchItem(target.id, {
        path: finalPath,
        fileName: finalName,
        recordedAt: parsed.recordedAt?.toISOString() ?? target.recordedAt,
        source: parsed.source ?? target.source,
      });
    } catch (err) {
      patchItem(target.id, { status: "error", error: String(err) });
    } finally {
      setRenameTarget(null);
    }
  };

  const selected = items.find((item) => item.id === selectedId) ?? null;
  const pendingCount = items.filter((item) => item.status !== "done").length;
  const allTags = Array.from(new Set(items.flatMap((item) => item.tags ?? []))).sort();
  const visibleItems =
    tagFilter && allTags.includes(tagFilter)
      ? items.filter((item) => item.tags?.includes(tagFilter))
      : items;

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center justify-between gap-4 border-b border-border bg-card px-4 py-2.5">
        <div className="flex items-center gap-2">
          <img src="/qlipq.svg" alt="" width={22} height={22} />
          <span className="font-bold tracking-tight">QlipQ</span>
          <button
            type="button"
            className="border-l border-border pl-2 text-[11px] text-muted-foreground hover:text-foreground"
            onClick={() => {
              api.openExternal(api.FFMPEG_URL).catch((err: unknown) => console.error(err));
            }}
          >
            Powered by FFmpeg
          </button>
        </div>
        <nav className="flex items-center gap-1">
          <Button
            variant={view === "queue" ? "secondary" : "ghost"}
            size="sm"
            onClick={() => setView("queue")}
          >
            Queue
            <Badge variant="secondary" className="ml-1">
              {pendingCount}
            </Badge>
          </Button>
          <Button
            variant={view === "settings" ? "secondary" : "ghost"}
            size="sm"
            onClick={() => setView("settings")}
          >
            Settings
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            title="View QlipQ on GitHub"
            aria-label="View QlipQ on GitHub"
            onClick={() => {
              api.openExternal(api.REPO_URL).catch((err: unknown) => console.error(err));
            }}
          >
            <svg viewBox="0 0 16 16" width={16} height={16} aria-hidden="true">
              <path
                fill="currentColor"
                d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 2-.27c.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8z"
              />
            </svg>
          </Button>
        </nav>
      </header>

      {view === "settings" ? (
        <main className="flex-1 overflow-y-auto p-6">
          <ConfigPanel
            config={config}
            presets={presets}
            onChange={patchConfig}
            onReprocess={reprocessFolder}
          />
        </main>
      ) : (
        <main className="grid min-h-0 flex-1 grid-cols-[340px_1fr]">
          <aside className="flex flex-col gap-2 overflow-y-auto border-r border-border p-3">
            {config.watchedFolders.length > 0 && (
              <div className="flex justify-end">
                <Button variant="link" size="sm" className="h-auto p-0" onClick={rescanAllFolders}>
                  Rescan all folders
                </Button>
              </div>
            )}
            {allTags.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                <Button
                  variant={tagFilter === null ? "secondary" : "ghost"}
                  size="xs"
                  onClick={() => setTagFilter(null)}
                >
                  All
                </Button>
                {allTags.map((tag) => (
                  <Button
                    key={tag}
                    variant={tagFilter === tag ? "secondary" : "ghost"}
                    size="xs"
                    onClick={() => setTagFilter(tag)}
                  >
                    {tag}
                  </Button>
                ))}
              </div>
            )}
            <QueueList
              items={visibleItems}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onRename={setRenameTarget}
              onRemove={removeItem}
              onDelete={requestDelete}
            />
            {ready && items.length === 0 && config.watchedFolders.length === 0 && (
              <Button variant="link" className="mt-2" onClick={() => setView("settings")}>
                Add a watched folder →
              </Button>
            )}
          </aside>
          <section className="min-w-0 overflow-y-auto">
            {selected ? (
              <Editor
                key={selected.id}
                item={selected}
                config={config}
                onPatch={patchItem}
                audioDefaults={audioDefaults}
                onAudioDefaults={setAudioDefaults}
              />
            ) : (
              <div className="flex h-full items-center justify-center p-6">
                <p className="text-sm text-muted-foreground">
                  Select a clip from the queue to start editing.
                </p>
              </div>
            )}
          </section>
        </main>
      )}

      {renameTarget && (
        <RenameModal
          item={renameTarget}
          namingTemplate={config.namingTemplate}
          onCancel={() => setRenameTarget(null)}
          onConfirm={confirmRename}
        />
      )}
      <AlertDialog
        open={deleteTarget != null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this file from disk?</AlertDialogTitle>
            <AlertDialogDescription>
              {deleteTarget?.fileName} will be permanently deleted from your drive. This can't be
              undone. (You won't be asked again.)
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={confirmDelete}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <Toaster />
    </div>
  );
}
