--[[
  QlipQRenamer

  Based on the original OBS Studio script by oxypatic:
      https://obsproject.com/forum/resources/recorder.1926/
  This is an independent Lua reimplementation of that script's idea. All credit
  for the original concept and design belongs to its author; see README.md for
  how this version differs from the original.

  Sorts finished recordings, saved replay-buffer clips, and screenshots into
  per-game subfolders (ShadowPlay-style) named after the application captured by
  the scene's Game Capture / Window Capture source (or, on Windows, the focused
  window). Files stay in their original output directory; they are only moved
  into subfolders of it.

  Drop this file in OBS via  Tools -> Scripts -> +  (Lua, no Python install
  needed). See README.md for behavior, settings, and known limitations.
]]

obs = obslua

local SEP = package.config:sub(1, 1) -- "\" on Windows, "/" elsewhere
local IS_WINDOWS = SEP == "\\"

local state = {
  fallback_name = "Any Recording",
  organization_mode = "basic", -- "basic" | "date"
  move_to_folders = true, -- move clips into per-game folders
  title_as_prefix = false,
  write_metadata = false, -- embed the game name as file metadata (via ffmpeg)
  ffmpeg_path = "ffmpeg", -- used only when write_metadata is on
  organize_replays = true,
  replay_folder = "", -- "" = replays go straight in the game folder (no subfolder)
  organize_screenshots = true,
  screenshot_folder = "screenshot",
  current_raw = nil, -- raw (unsanitized) title of the detected app, or nil
  current_method = nil, -- how it was detected, for the naming log
}

-- Mirror the original's title sanitizing: keep [A-Za-z0-9 ], collapse spaces, trim.
local function sanitize_title(title)
  if not title or title == "" then return "" end
  title = title:gsub("[^A-Za-z0-9 ]", "")
  title = title:gsub("%s+", " ")
  return (title:match("^%s*(.-)%s*$"))
end

-- Resolve a single source's captured-app title. Returns (title, method) where
-- method describes how it was found (for the naming log), or nil if no match.
local function source_title(src)
  local id = obs.obs_source_get_unversioned_id(src)
  if id == "game_capture" then
    -- proc: void get_hooked(out bool hooked, out string title, ...). The
    -- calldata is ours here (manual call) so we must destroy it.
    local ph = obs.obs_source_get_proc_handler(src) -- borrowed, do not free
    local cd = obs.calldata_create()
    obs.proc_handler_call(ph, "get_hooked", cd)
    local hooked = obs.calldata_bool(cd, "hooked")
    local title = obs.calldata_string(cd, "title")
    obs.calldata_destroy(cd)
    if hooked and title and title ~= "" then
      return title, ("Game Capture '%s' (hooked)"):format(obs.obs_source_get_name(src) or "?")
    end
  elseif id == "window_capture" then
    local settings = obs.obs_source_get_settings(src) -- increments refcount
    local window = obs.obs_data_get_string(settings, "window")
    obs.obs_data_release(settings) -- required
    -- "window" is "title:class:executable"; the title is the first field.
    if window and window ~= "" then
      local title = window:match("^(.-):")
      if title and title ~= "" then
        return title, ("Window Capture '%s'"):format(obs.obs_source_get_name(src) or "?")
      end
    end
  end
  return nil
end

-- Foreground-window detection (Windows only), so Display Capture recordings can
-- still be sorted by the focused app. Uses LuaJIT FFI into user32/kernel32; stays
-- a no-op on non-Windows OBS builds or if FFI is unavailable.
local foreground_app_title = function() return nil end
do
  local ok, ffi = pcall(require, "ffi")
  if ok and ffi.os == "Windows" then
    local built_ok, resolver = pcall(function()
      ffi.cdef([[
        typedef void* HWND; typedef void* HANDLE; typedef unsigned long DWORD;
        HWND   GetForegroundWindow(void);
        int    GetWindowTextA(HWND hWnd, char* lpString, int nMaxCount);
        DWORD  GetWindowThreadProcessId(HWND hWnd, DWORD* lpdwProcessId);
        HANDLE OpenProcess(DWORD dwDesiredAccess, int bInheritHandle, DWORD dwProcessId);
        int    QueryFullProcessImageNameA(HANDLE hProcess, DWORD dwFlags, char* lpExeName, DWORD* lpdwSize);
        int    CloseHandle(HANDLE hObject);
      ]])
      local user32 = ffi.load("user32")
      local kernel32 = ffi.load("kernel32")
      local QUERY_LIMITED = 0x1000 -- PROCESS_QUERY_LIMITED_INFORMATION
      -- Never sort by our own window or the desktop shell.
      local ignored = { ["obs64.exe"] = true, ["obs32.exe"] = true, ["obs.exe"] = true, ["explorer.exe"] = true }

      local function exe_basename(hwnd)
        local pid = ffi.new("DWORD[1]")
        user32.GetWindowThreadProcessId(hwnd, pid)
        local proc = kernel32.OpenProcess(QUERY_LIMITED, 0, pid[0])
        if proc == nil then return nil end
        local buf, size = ffi.new("char[?]", 260), ffi.new("DWORD[1]", 260)
        local got = kernel32.QueryFullProcessImageNameA(proc, 0, buf, size) ~= 0
        kernel32.CloseHandle(proc)
        if not got then return nil end
        return (ffi.string(buf, size[0]):match("[^/\\]+$"))
      end

      return function()
        local hwnd = user32.GetForegroundWindow()
        if hwnd == nil then return nil end
        local exe = exe_basename(hwnd)
        if exe and ignored[exe:lower()] then return nil end
        local method = ("foreground window (%s)"):format(exe or "?")
        local buf = ffi.new("char[?]", 512)
        local len = user32.GetWindowTextA(hwnd, buf, 512)
        if len > 0 then return ffi.string(buf, len), method end
        if exe then return (exe:gsub("%.[Ee][Xx][Ee]$", "")), method end
        return nil
      end
    end)
    if built_ok then foreground_app_title = resolver end
  end
end

-- Detect the captured app: a visible Game/Window Capture source in the current
-- scene, else (e.g. Display Capture) the OS foreground window. Returns
-- (title, method); (nil, nil) if nothing usable was found.
local function detect_current_title()
  local title, method = nil, nil
  local scene_source = obs.obs_frontend_get_current_scene() -- new ref -> release
  if scene_source ~= nil then
    local scene = obs.obs_scene_from_source(scene_source) -- borrowed
    if scene ~= nil then
      local items = obs.obs_scene_enum_items(scene) -- list -> must release
      for _, item in ipairs(items) do
        if obs.obs_sceneitem_visible(item) then
          local src = obs.obs_sceneitem_get_source(item) -- borrowed
          if src ~= nil then
            local t, m = source_title(src)
            if t then title, method = t, m break end
          end
        end
      end
      obs.sceneitem_list_release(items)
    end
    obs.obs_source_release(scene_source)
  end
  if not title then title, method = foreground_app_title() end
  return title, method
end

-- Keep the detected app fresh; cleared when nothing is detected so a desktop
-- screenshot doesn't land in the last game's folder.
local function poll()
  local raw, method = detect_current_title()
  state.current_raw = raw and raw ~= "" and raw or nil
  state.current_method = state.current_raw and method or nil
end

local function dir_of(path) return path:match("^(.*)[/\\][^/\\]*$") end
local function name_of(path) return path:match("[^/\\]+$") end

local function build_dest_dir(base_dir, game, kind)
  local parts = { base_dir, game }
  -- An empty subfolder name means "no subfolder" — skip it so we don't leave a
  -- stray path separator.
  if kind == "replay" and state.replay_folder ~= "" then
    parts[#parts + 1] = state.replay_folder
  elseif kind == "screenshot" and state.screenshot_folder ~= "" then
    parts[#parts + 1] = state.screenshot_folder
  end
  if state.organization_mode == "date" then
    parts[#parts + 1] = os.date("%y-%m-%d")
  end
  return table.concat(parts, SEP)
end

-- Log, step by step, how this file's destination name was decided.
local function log_naming(kind, src_path, raw, method, game, fallback, layout, dest_path)
  local lines = { ("QlipQRenamer: %s '%s'"):format(kind, name_of(src_path)) }
  if fallback then
    if raw and raw ~= "" then
      lines[#lines + 1] = ("  app:    %s -> '%s', sanitized to empty -> fallback '%s'"):format(
        method or "?", raw, state.fallback_name)
    else
      lines[#lines + 1] = ("  app:    none detected -> fallback '%s'"):format(state.fallback_name)
    end
  else
    lines[#lines + 1] = ("  app:    %s -> '%s'"):format(method or "?", raw or "")
    if raw ~= game then
      lines[#lines + 1] = ("  name:   sanitized -> '%s'"):format(game)
    end
  end
  if state.move_to_folders then
    lines[#lines + 1] = "  layout: " .. layout .. " (subfolders under the recording folder)"
  else
    lines[#lines + 1] = "  layout: not moved (kept in its original folder)"
  end
  if state.title_as_prefix then
    lines[#lines + 1] = ("  prefix: filename prefixed with '%s'"):format(game)
  end
  if state.write_metadata then
    lines[#lines + 1] = ("  metadata: game = '%s' (requested via ffmpeg)"):format(game)
  end
  lines[#lines + 1] = "  result: " .. dest_path
  obs.script_log(obs.LOG_INFO, table.concat(lines, "\n"))
end

-- Wrap one command-line arg in quotes (game names are sanitized to [A-Za-z0-9 ];
-- paths may contain spaces — neither contains quotes).
local function quote(s) return '"' .. s .. '"' end

-- Run a pre-quoted command synchronously; true on exit 0. On Windows the whole
-- command gets an extra outer quote pair because `cmd /c` strips one (verified:
-- without it, a program path containing a space is split and fails).
local function run(args)
  local cmd = table.concat(args, " ")
  if IS_WINDOWS then cmd = '"' .. cmd .. '"' end
  local res = os.execute(cmd)
  if type(res) == "number" then return res == 0 end
  return res == true
end

-- Stream-copy src -> dest, embedding the game as container metadata (no re-encode).
local function write_with_metadata(src, dest, game)
  return run({
    quote(state.ffmpeg_path), "-y", "-hide_banner", "-loglevel", "error",
    "-i", quote(src), "-map", "0", "-c", "copy",
    "-metadata", quote("game=" .. game), quote(dest),
  })
end

-- Produce a metadata-tagged copy at dest and drop the original. Never destroys the
-- source: ffmpeg writes a temp first, and any failure falls back to a plain move
-- (or leaves the file untouched).
local function tag_and_place(src, dest, dest_dir, game)
  obs.os_mkdirs(dest_dir)
  -- FFmpeg infers the output container from the final extension.
  local stem, extension = dest:match("^(.*)(%.[^./\\]+)$")
  local tmp = (stem or dest) .. ".qqtmp" .. (extension or "")
  obs.os_unlink(tmp)
  if not write_with_metadata(src, tmp, game) or not obs.os_file_exists(tmp) then
    obs.os_unlink(tmp)
    obs.script_log(obs.LOG_WARNING,
      "QlipQRenamer: ffmpeg metadata write failed (check the ffmpeg path / container support); original kept: " .. src)
    if dest ~= src then
      if obs.os_rename(src, dest) == 0 then
        obs.script_log(obs.LOG_INFO, "QlipQRenamer: moved (untagged) to " .. dest)
      else
        obs.script_log(obs.LOG_WARNING, "QlipQRenamer: fallback move also failed; left at " .. src)
      end
    end
    return
  end
  if dest == src then
    local bak = src .. ".qqbak"
    obs.os_unlink(bak)
    if obs.os_rename(src, bak) ~= 0 then
      obs.os_unlink(tmp)
      obs.script_log(obs.LOG_WARNING, "QlipQRenamer: could not back up original; left unchanged: " .. src)
      return
    end
    if obs.os_rename(tmp, dest) == 0 then
      if obs.os_unlink(bak) ~= 0 then
        obs.script_log(obs.LOG_WARNING, "QlipQRenamer: tagged, but could not remove backup: " .. bak)
      end
      obs.script_log(obs.LOG_INFO, "QlipQRenamer: tagged in place: " .. dest)
    else
      obs.os_rename(bak, src) -- restore the original
      obs.os_unlink(tmp)
      obs.script_log(obs.LOG_WARNING, "QlipQRenamer: could not replace original (restored): " .. src)
    end
  else
    if obs.os_rename(tmp, dest) == 0 then
      obs.os_unlink(src)
      obs.script_log(obs.LOG_INFO, "QlipQRenamer: tagged + moved to " .. dest)
    else
      obs.os_unlink(tmp)
      obs.script_log(obs.LOG_WARNING, "QlipQRenamer: could not place tagged file (original kept): " .. src)
    end
  end
end

local function move_file(src_path, kind)
  if not src_path or src_path == "" then
    obs.script_log(obs.LOG_WARNING, ("QlipQRenamer: %s — OBS reported no file path (nothing to move)"):format(kind))
    return
  end
  if not obs.os_file_exists(src_path) then
    obs.script_log(obs.LOG_WARNING, ("QlipQRenamer: %s — file not on disk yet: %s"):format(kind, src_path))
    return
  end

  local base_dir = dir_of(src_path)
  local filename = name_of(src_path)
  if not base_dir or not filename then return end

  -- Resolve the app title and remember how it was found (for the log).
  local raw, method = state.current_raw, state.current_method
  if not raw or raw == "" then
    raw, method = detect_current_title() -- last-chance re-resolve
  end
  local game = sanitize_title(raw or "")
  local fallback = game == ""
  if fallback then game = state.fallback_name end

  local dest_dir = state.move_to_folders and build_dest_dir(base_dir, game, kind) or base_dir
  local out_name = state.title_as_prefix and (game .. " - " .. filename) or filename
  local dest_path = dest_dir .. SEP .. out_name
  if not state.move_to_folders and not state.title_as_prefix then dest_path = src_path end
  local layout = dest_dir:sub(#base_dir + 2) -- subfolder structure under the recording dir

  log_naming(kind, src_path, raw, method, game, fallback, layout, dest_path)

  -- Refuse to clobber an existing different file.
  if dest_path ~= src_path and obs.os_file_exists(dest_path) then
    obs.script_log(obs.LOG_WARNING, "QlipQRenamer: destination exists, leaving in place: " .. dest_path)
    return
  end

  if state.write_metadata then
    tag_and_place(src_path, dest_path, dest_dir, game)
  elseif dest_path ~= src_path then
    obs.os_mkdirs(dest_dir)
    -- dest is under the file's own directory, so this is a same-volume rename.
    if obs.os_rename(src_path, dest_path) == 0 then
      obs.script_log(obs.LOG_INFO, "QlipQRenamer: moved to " .. dest_path)
    else
      obs.script_log(obs.LOG_WARNING, "QlipQRenamer: failed to move " .. src_path)
    end
  else
    obs.script_log(obs.LOG_INFO, "QlipQRenamer: left in place (no move, no metadata): " .. src_path)
  end
end

-- Resolve the just-saved replay's path. The frontend helper can be empty right
-- after the SAVED event, so fall back to the replay output's get_last_replay proc.
-- Every binding is guarded so this can never raise (OBS would swallow the error).
-- Returns (path, via) — `via` names the source (or the reason it's nil) for the log.
local function last_replay_path()
  if type(obs.obs_frontend_get_last_replay) == "function" then
    local p = obs.obs_frontend_get_last_replay()
    if p and p ~= "" then return p, "frontend" end
  end
  if type(obs.obs_frontend_get_replay_buffer_output) ~= "function" then
    return nil, "no replay-output binding"
  end
  local rb = obs.obs_frontend_get_replay_buffer_output()
  if rb == nil then return nil, "replay output is nil (buffer inactive?)" end
  local p
  local ph = obs.obs_output_get_proc_handler(rb)
  if ph ~= nil then
    local cd = obs.calldata_create()
    obs.proc_handler_call(ph, "get_last_replay", cd)
    p = obs.calldata_string(cd, "path")
    obs.calldata_destroy(cd)
  end
  obs.obs_output_release(rb)
  if p and p ~= "" then return p, "proc" end
  return nil, "empty path from proc"
end

local function on_event(event)
  if event == obs.OBS_FRONTEND_EVENT_RECORDING_STOPPED then
    move_file(obs.obs_frontend_get_last_recording(), "recording")
  elseif event == obs.OBS_FRONTEND_EVENT_REPLAY_BUFFER_SAVED then
    -- Always log on the event so we can see whether it fires at all.
    if not state.organize_replays then
      obs.script_log(obs.LOG_INFO, "QlipQRenamer: replay saved, but 'Organize replay-buffer saves' is off")
      return
    end
    local ok, path, via = pcall(last_replay_path)
    if not ok then
      obs.script_log(obs.LOG_WARNING, "QlipQRenamer: replay path lookup errored: " .. tostring(path))
      return
    end
    obs.script_log(obs.LOG_INFO, ("QlipQRenamer: replay saved (path via %s): %s"):format(tostring(via), tostring(path)))
    move_file(path, "replay")
  elseif event == obs.OBS_FRONTEND_EVENT_SCREENSHOT_TAKEN then
    if state.organize_screenshots then
      move_file(obs.obs_frontend_get_last_screenshot(), "screenshot")
    end
  end
end

function script_description()
  return [[<b>QlipQRenamer</b><br/>
Sorts recordings, replay-buffer saves, and screenshots into per-game folders,
ShadowPlay-style, using the scene's Game Capture / Window Capture source.<br/><br/>
Based on the original <a href="https://obsproject.com/forum/resources/recorder.1926/">OBS Studio script by oxypatic</a> — reimplemented in Lua (no Python needed).]]
end

function script_properties()
  local p = obs.obs_properties_create()
  obs.obs_properties_add_text(p, "fallback_name", "Fallback folder name", obs.OBS_TEXT_DEFAULT)

  local mode = obs.obs_properties_add_list(p, "organization_mode", "Organization mode",
    obs.OBS_COMBO_TYPE_LIST, obs.OBS_COMBO_FORMAT_STRING)
  obs.obs_property_list_add_string(mode, "Folder per game", "basic")
  obs.obs_property_list_add_string(mode, "Folder per game, then date", "date")

  obs.obs_properties_add_bool(p, "move_to_folders", "Move clips into per-game folders")
  obs.obs_properties_add_bool(p, "title_as_prefix", "Prefix filenames with the game title")
  obs.obs_properties_add_bool(p, "write_metadata", "Write the game name into file metadata (uses ffmpeg)")
  obs.obs_properties_add_text(p, "ffmpeg_path", "ffmpeg path (for metadata)", obs.OBS_TEXT_DEFAULT)
  obs.obs_properties_add_bool(p, "organize_replays", "Organize replay-buffer saves")
  obs.obs_properties_add_text(p, "replay_folder", "Replay subfolder name", obs.OBS_TEXT_DEFAULT)
  obs.obs_properties_add_bool(p, "organize_screenshots", "Organize screenshots")
  obs.obs_properties_add_text(p, "screenshot_folder", "Screenshot subfolder name", obs.OBS_TEXT_DEFAULT)
  return p
end

function script_defaults(settings)
  obs.obs_data_set_default_string(settings, "fallback_name", "Any Recording")
  obs.obs_data_set_default_string(settings, "organization_mode", "basic")
  obs.obs_data_set_default_bool(settings, "move_to_folders", true)
  obs.obs_data_set_default_bool(settings, "title_as_prefix", false)
  obs.obs_data_set_default_bool(settings, "write_metadata", false)
  obs.obs_data_set_default_string(settings, "ffmpeg_path", "ffmpeg")
  obs.obs_data_set_default_bool(settings, "organize_replays", true)
  obs.obs_data_set_default_string(settings, "replay_folder", "")
  obs.obs_data_set_default_bool(settings, "organize_screenshots", true)
  obs.obs_data_set_default_string(settings, "screenshot_folder", "screenshot")
end

function script_update(settings)
  state.fallback_name = obs.obs_data_get_string(settings, "fallback_name")
  state.organization_mode = obs.obs_data_get_string(settings, "organization_mode")
  state.move_to_folders = obs.obs_data_get_bool(settings, "move_to_folders")
  state.title_as_prefix = obs.obs_data_get_bool(settings, "title_as_prefix")
  state.write_metadata = obs.obs_data_get_bool(settings, "write_metadata")
  state.ffmpeg_path = obs.obs_data_get_string(settings, "ffmpeg_path")
  state.organize_replays = obs.obs_data_get_bool(settings, "organize_replays")
  state.replay_folder = obs.obs_data_get_string(settings, "replay_folder")
  state.organize_screenshots = obs.obs_data_get_bool(settings, "organize_screenshots")
  state.screenshot_folder = obs.obs_data_get_string(settings, "screenshot_folder")
end

function script_load(settings)
  obs.obs_frontend_add_event_callback(on_event)
  obs.timer_add(poll, 1500)
end

function script_unload()
  obs.timer_remove(poll)
  obs.obs_frontend_remove_event_callback(on_event)
end
