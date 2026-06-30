--[[
  RecORDER (Lua) — a lightweight Lua port of the RecORDER OBS Python script.

  Sorts finished recordings, saved replay-buffer clips, and screenshots into
  per-game subfolders (ShadowPlay-style) named after the application captured by
  the scene's Game Capture / Window Capture source. Files stay in their original
  output directory; they are only moved into subfolders of it.

  Drop this file in OBS via  Tools -> Scripts -> +  (Lua, no Python install
  needed). See README.md for behavior, settings, and known limitations.
]]

obs = obslua

local SEP = package.config:sub(1, 1) -- "\" on Windows, "/" elsewhere

local state = {
  fallback_name = "Any Recording",
  organization_mode = "basic", -- "basic" | "date"
  title_as_prefix = false,
  organize_replays = true,
  replay_folder = "replay",
  organize_screenshots = true,
  screenshot_folder = "screenshot",
  current_title = nil, -- sanitized title of the currently hooked app, or nil
}

-- Match RecORDER's __sanitizeTitle: keep [A-Za-z0-9 ], collapse spaces, trim.
local function sanitize_title(title)
  if not title or title == "" then return "" end
  title = title:gsub("[^A-Za-z0-9 ]", "")
  title = title:gsub("%s+", " ")
  return (title:match("^%s*(.-)%s*$"))
end

-- Resolve the captured app title from a single source, or nil.
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
    if hooked and title and title ~= "" then return title end
  elseif id == "window_capture" then
    local settings = obs.obs_source_get_settings(src) -- increments refcount
    local window = obs.obs_data_get_string(settings, "window")
    obs.obs_data_release(settings) -- required
    -- "window" is "title:class:executable"; the title is the first field.
    if window and window ~= "" then
      local title = window:match("^(.-):")
      if title and title ~= "" then return title end
    end
  end
  return nil
end

-- Title of the app captured by a visible source in the current scene, or nil.
local function current_scene_title()
  local scene_source = obs.obs_frontend_get_current_scene() -- new ref -> release
  if scene_source == nil then return nil end
  local title = nil
  local scene = obs.obs_scene_from_source(scene_source) -- borrowed
  if scene ~= nil then
    local items = obs.obs_scene_enum_items(scene) -- list -> must release
    for _, item in ipairs(items) do
      if obs.obs_sceneitem_visible(item) then
        local src = obs.obs_sceneitem_get_source(item) -- borrowed
        if src ~= nil then
          local t = source_title(src)
          if t then title = t break end
        end
      end
    end
    obs.sceneitem_list_release(items)
  end
  obs.obs_source_release(scene_source)
  return title
end

-- Keep state.current_title fresh; cleared when nothing is hooked so a
-- desktop screenshot doesn't land in the last game's folder.
local function poll()
  local t = current_scene_title()
  state.current_title = t and sanitize_title(t) or nil
end

local function dir_of(path) return path:match("^(.*)[/\\][^/\\]*$") end
local function name_of(path) return path:match("[^/\\]+$") end

local function build_dest_dir(base_dir, game, kind)
  local parts = { base_dir, game }
  if kind == "replay" then
    parts[#parts + 1] = state.replay_folder
  elseif kind == "screenshot" then
    parts[#parts + 1] = state.screenshot_folder
  end
  if state.organization_mode == "date" then
    parts[#parts + 1] = os.date("%y-%m-%d")
  end
  return table.concat(parts, SEP)
end

local function move_file(src_path, kind)
  if not src_path or src_path == "" then return end
  if not obs.os_file_exists(src_path) then return end

  local base_dir = dir_of(src_path)
  local filename = name_of(src_path)
  if not base_dir or not filename then return end

  local game = state.current_title
  if not game or game == "" then
    game = sanitize_title(current_scene_title() or "") -- last-chance resolve
  end
  if game == "" then game = state.fallback_name end

  local dest_dir = build_dest_dir(base_dir, game, kind)
  obs.os_mkdirs(dest_dir)

  if state.title_as_prefix then filename = game .. " - " .. filename end
  local dest_path = dest_dir .. SEP .. filename

  if obs.os_file_exists(dest_path) then
    obs.script_log(obs.LOG_WARNING, "RecORDER: destination exists, leaving in place: " .. dest_path)
    return
  end
  -- dest is a subfolder of the file's own directory, so this is always a
  -- same-volume rename (atomic, instant) -- no cross-device copy needed.
  if obs.os_rename(src_path, dest_path) == 0 then
    obs.script_log(obs.LOG_INFO, "RecORDER: moved to " .. dest_path)
  else
    obs.script_log(obs.LOG_WARNING, "RecORDER: failed to move " .. src_path)
  end
end

local function on_event(event)
  if event == obs.OBS_FRONTEND_EVENT_RECORDING_STOPPED then
    move_file(obs.obs_frontend_get_last_recording(), "recording")
  elseif event == obs.OBS_FRONTEND_EVENT_REPLAY_BUFFER_SAVED then
    if state.organize_replays then
      move_file(obs.obs_frontend_get_last_replay(), "replay")
    end
  elseif event == obs.OBS_FRONTEND_EVENT_SCREENSHOT_TAKEN then
    if state.organize_screenshots then
      move_file(obs.obs_frontend_get_last_screenshot(), "screenshot")
    end
  end
end

function script_description()
  return [[<b>RecORDER (Lua)</b><br/>
Sorts recordings, replay-buffer saves, and screenshots into per-game folders,
ShadowPlay-style, using the scene's Game Capture / Window Capture source.<br/>
A lightweight Lua port of the RecORDER Python script — no Python install needed.]]
end

function script_properties()
  local p = obs.obs_properties_create()
  obs.obs_properties_add_text(p, "fallback_name", "Fallback folder name", obs.OBS_TEXT_DEFAULT)

  local mode = obs.obs_properties_add_list(p, "organization_mode", "Organization mode",
    obs.OBS_COMBO_TYPE_LIST, obs.OBS_COMBO_FORMAT_STRING)
  obs.obs_property_list_add_string(mode, "Folder per game", "basic")
  obs.obs_property_list_add_string(mode, "Folder per game, then date", "date")

  obs.obs_properties_add_bool(p, "title_as_prefix", "Prefix filenames with the game title")
  obs.obs_properties_add_bool(p, "organize_replays", "Organize replay-buffer saves")
  obs.obs_properties_add_text(p, "replay_folder", "Replay subfolder name", obs.OBS_TEXT_DEFAULT)
  obs.obs_properties_add_bool(p, "organize_screenshots", "Organize screenshots")
  obs.obs_properties_add_text(p, "screenshot_folder", "Screenshot subfolder name", obs.OBS_TEXT_DEFAULT)
  return p
end

function script_defaults(settings)
  obs.obs_data_set_default_string(settings, "fallback_name", "Any Recording")
  obs.obs_data_set_default_string(settings, "organization_mode", "basic")
  obs.obs_data_set_default_bool(settings, "title_as_prefix", false)
  obs.obs_data_set_default_bool(settings, "organize_replays", true)
  obs.obs_data_set_default_string(settings, "replay_folder", "replay")
  obs.obs_data_set_default_bool(settings, "organize_screenshots", true)
  obs.obs_data_set_default_string(settings, "screenshot_folder", "screenshot")
end

function script_update(settings)
  state.fallback_name = obs.obs_data_get_string(settings, "fallback_name")
  state.organization_mode = obs.obs_data_get_string(settings, "organization_mode")
  state.title_as_prefix = obs.obs_data_get_bool(settings, "title_as_prefix")
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
