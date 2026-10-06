# UI-thread audit

The UI update loop changes application state and schedules tasks. File access, native media work and
network requests run outside it. Tests register the UI thread and assert that blocking entry points
are never called from startup, updates or widget processing.

| Work | Execution boundary | Regression evidence |
| --- | --- | --- |
| Configuration, schema and cached edits on startup | `App::new` schedules a blocking task; the window initially shows loading state | UI startup runs with the thread guard enabled |
| Config/edit/cache serialization and writes | `background::save` sends snapshots to one FIFO writer thread | A stalled write cannot block the UI or let a newer write overtake it |
| Folder scanning, file metadata and capture-preset detection | Blocking tasks; watcher startup and replacement also happen there | Add/remove/reprocess/real watcher E2E flows |
| Probe and scrub decoder initialization, seeking and HDR processing | Blocking tasks; session/generation tokens reject outdated results | Stalled media-read test can open Settings; same-clip stale-probe regression |
| Streaming preview and audio mixing | Initialization on the blocking pool; decode/audio run on dedicated workers; cpal consumes a ring buffer | Real playback, seeking, pause, hover and settings E2E flows |
| Decoder shutdown and thread joins | `background::Media` transfers native resource destruction to a worker; hover shutdown joins off the UI thread | A deliberately stalled destructor returns immediately to the UI |
| File rename/delete/move | Blocking tasks; a per-file read/write gate waits for preview, probe, thumbnail, highlight and export handles to close | Rename/delete and all after-export actions verify the resulting files and queue |
| Export planning, decode/filter/encode/mux and destination checks | Blocking tasks; cancellation is atomic and progress is read with `try_lock` | Stalled-export test can cancel immediately; real remux/AAC export and optional hardware crop export |
| Highlight analysis | Frame decode and PNG encoding on the blocking pool; Ollama HTTP requests on the async executor | Real frame sampling plus HTTP-boundary responses; cancel and switch clips while a request is held open |
| OS file/browser launch | Worker thread; folder picker uses its asynchronous API | Requested paths and dialog cancellation are tested at the OS boundary |

The UI never locks the scrub decoder. It polls decoded frames and watcher notifications with
`try_lock`, skipping a busy producer instead of waiting. Session tokens prevent a late decoder or
probe from replacing the current editor, including when the same recording is selected again.
Rename/delete wait for native handles on workers. The exporting clip cannot be switched, dismissed,
renamed or deleted while its progress/cancel controls are active.

UI layout, queue filtering/sorting, small state snapshots and GPU frame upload/submission still belong
to rendering. Clock locks contain only scalar reads/writes; media decoding and file operations do not
hold them. This audit does not promise a frame-time ceiling for arbitrarily large libraries or a
malfunctioning GPU driver. Tests enforce a 100 ms ceiling on the exercised UI updates and deliberately
stall media reads, export and decoder shutdown to verify that their duration does not delay input.

## Validation scope

Run `cargo test` for the standard suite and
`cargo test -p qlipq-desktop --bin qlipq ui_e2e_hardware_crop_export -- --ignored` on a machine with a
supported encoder. Both were exercised on Windows during this change. CI runs the standard suite on
Windows and Linux, retaining rendered screenshots. Physical audio output, native window-system/dialog
behavior, HDR displays, other GPU vendors and Ollama model accuracy are outside the headless suite.
