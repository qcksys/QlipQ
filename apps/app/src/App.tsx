import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type AppConfig,
  DEFAULT_CONFIG,
  parseObsFilename,
  type QueueItem,
} from "@qcksys/qlipq-core";
import { toast } from "sonner";
import { ConfigPanel } from "./components/ConfigPanel.tsx";
import { Editor } from "./components/Editor.tsx";
import { QueueList } from "./components/QueueList.tsx";
import { RenameModal } from "./components/RenameModal.tsx";
import * as api from "./lib/api.ts";
import { basename, dirname, joinPath, queueItemFromPath, toPosixPath } from "./lib/queue.ts";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";

type View = "queue" | "settings";

/** Human-readable summary of a (re)scan: how many clips were newly queued. */
function describeScan(added: number, scanned: number, folder?: string): string {
  const where = folder ? ` in ${folder}` : "";
  if (added > 0) return `Added ${added} new clip${added === 1 ? "" : "s"}${where}.`;
  if (scanned === 0) return `No video files found${where}.`;
  return `No new clips${where} — all ${scanned} already in the queue.`;
}

export function App() {
  const [savedConfig, setSavedConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [items, setItems] = useState<QueueItem[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("queue");
  const [renameTarget, setRenameTarget] = useState<QueueItem | null>(null);
  const [ready, setReady] = useState(false);
  const [presets, setPresets] = useState<api.CapturePresets>({});

  // Avoid duplicate queue entries for the same path (scan + watcher overlap).
  const knownPaths = useRef(new Set<string>());

  // Dedup and ref-mutation happen here (outside setItems) so the result is correct
  // under StrictMode, which invokes state updaters twice. Returns how many were added.
  const addPaths = useCallback((paths: string[]): number => {
    const fresh: string[] = [];
    for (const raw of paths) {
      const path = toPosixPath(raw);
      if (!knownPaths.current.has(path)) {
        knownPaths.current.add(path);
        fresh.push(path);
      }
    }
    if (fresh.length === 0) return 0;
    const additions = fresh.map((path) => queueItemFromPath(path, new Date().toISOString()));
    setItems((current) => [...additions, ...current]);
    return fresh.length;
  }, []);

  const loadFromFolders = useCallback(
    async (cfg: AppConfig) => {
      const found = await api.scanFolders(cfg.watchedFolders, cfg.videoExtensions);
      addPaths(found);
      await api.startWatching(cfg.watchedFolders, cfg.videoExtensions);
    },
    [addPaths],
  );

  // Initial load: config, queue population, watcher subscription.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const loaded = await api.getConfig();
      setSavedConfig(loaded);
      setConfig(loaded);
      unlisten = await api.onFileAdded((path) => addPaths([path]));
      await loadFromFolders(loaded);
      setReady(true);
      // Best-effort; detectCapturePresets resolves to {} if a source is unavailable.
      api
        .detectCapturePresets()
        .then(setPresets, (err: unknown) => console.error("preset detection failed", err));
    })().catch((err: unknown) => console.error("startup failed", err));
    return () => unlisten?.();
  }, [addPaths, loadFromFolders]);

  const patchConfig = (patch: Partial<AppConfig>) => setConfig((c) => ({ ...c, ...patch }));

  const saveConfig = async () => {
    await api.setConfig(config);
    setSavedConfig(config);
    await loadFromFolders(config);
  };

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

  const patchItem = useCallback((id: string, patch: Partial<QueueItem>) => {
    setItems((current) => current.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  }, []);

  const removeItem = (id: string) => {
    setItems((current) => {
      const target = current.find((item) => item.id === id);
      if (target) knownPaths.current.delete(target.path);
      return current.filter((item) => item.id !== id);
    });
    if (selectedId === id) setSelectedId(null);
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

  const dirty = useMemo(
    () => JSON.stringify(config) !== JSON.stringify(savedConfig),
    [config, savedConfig],
  );

  const selected = items.find((item) => item.id === selectedId) ?? null;
  const pendingCount = items.filter((item) => item.status !== "done").length;

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <header className="flex items-center justify-between gap-4 border-b border-border bg-card px-4 py-2.5">
        <div className="flex items-center gap-2">
          <img src="/qlipq.svg" alt="" width={22} height={22} />
          <span className="font-bold tracking-tight">QlipQ</span>
          <span className="border-l border-border pl-2 text-[11px] text-muted-foreground">
            Powered by FFmpeg
          </span>
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
            {dirty && (
              <span className="ml-1 size-1.5 rounded-full bg-primary" aria-label="unsaved" />
            )}
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
            dirty={dirty}
            presets={presets}
            onChange={patchConfig}
            onSave={saveConfig}
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
            <QueueList
              items={items}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onRename={setRenameTarget}
              onRemove={removeItem}
            />
            {ready && items.length === 0 && config.watchedFolders.length === 0 && (
              <Button variant="link" className="mt-2" onClick={() => setView("settings")}>
                Add a watched folder →
              </Button>
            )}
          </aside>
          <section className="min-w-0 overflow-y-auto">
            {selected ? (
              <Editor key={selected.id} item={selected} config={config} onPatch={patchItem} />
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
      <Toaster />
    </div>
  );
}
