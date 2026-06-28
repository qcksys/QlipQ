import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type AppConfig, DEFAULT_CONFIG, parseObsFilename, type QueueItem } from "@qlipq/core";
import { ConfigPanel } from "./components/ConfigPanel.tsx";
import { Editor } from "./components/Editor.tsx";
import { QueueList } from "./components/QueueList.tsx";
import { RenameModal } from "./components/RenameModal.tsx";
import * as api from "./lib/api.ts";
import { basename, dirname, joinPath, queueItemFromPath } from "./lib/queue.ts";

type View = "queue" | "settings";

export function App() {
  const [savedConfig, setSavedConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [items, setItems] = useState<QueueItem[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("queue");
  const [renameTarget, setRenameTarget] = useState<QueueItem | null>(null);
  const [ready, setReady] = useState(false);

  // Avoid duplicate queue entries for the same path (scan + watcher overlap).
  const knownPaths = useRef(new Set<string>());

  const addPaths = useCallback((paths: string[]) => {
    setItems((current) => {
      const additions = paths
        .filter((path) => !knownPaths.current.has(path))
        .map((path) => {
          knownPaths.current.add(path);
          return queueItemFromPath(path, new Date().toISOString());
        });
      return additions.length ? [...additions, ...current] : current;
    });
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
    })().catch((err: unknown) => console.error("startup failed", err));
    return () => unlisten?.();
  }, [addPaths, loadFromFolders]);

  const patchConfig = (patch: Partial<AppConfig>) => setConfig((c) => ({ ...c, ...patch }));

  const saveConfig = async () => {
    await api.setConfig(config);
    setSavedConfig(config);
    await loadFromFolders(config);
  };

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
    <div className="app">
      <header className="app-header">
        <div className="brand">
          <img src="/qlipq.svg" alt="" width={22} height={22} />
          <span>qlipq</span>
        </div>
        <nav className="tabs">
          <button
            type="button"
            className={view === "queue" ? "active" : ""}
            onClick={() => setView("queue")}
          >
            Queue <span className="count">{pendingCount}</span>
          </button>
          <button
            type="button"
            className={view === "settings" ? "active" : ""}
            onClick={() => setView("settings")}
          >
            Settings {dirty && <span className="dot" aria-label="unsaved changes" />}
          </button>
        </nav>
      </header>

      {view === "settings" ? (
        <main className="settings-view">
          <ConfigPanel config={config} dirty={dirty} onChange={patchConfig} onSave={saveConfig} />
        </main>
      ) : (
        <main className="queue-view">
          <aside className="queue-pane">
            <QueueList
              items={items}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onRename={setRenameTarget}
              onRemove={removeItem}
            />
            {ready && items.length === 0 && config.watchedFolders.length === 0 && (
              <button type="button" className="link cta" onClick={() => setView("settings")}>
                Add a watched folder →
              </button>
            )}
          </aside>
          <section className="editor-pane">
            {selected ? (
              <Editor key={selected.id} item={selected} config={config} onPatch={patchItem} />
            ) : (
              <div className="editor empty">
                <p className="muted">Select a clip from the queue to start editing.</p>
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
    </div>
  );
}
