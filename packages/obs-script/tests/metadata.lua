-- Run from the repository root: luajit packages/obs-script/tests/metadata.lua
-- Requires ffmpeg (with libaom-av1) and ffprobe on PATH.
local windows = package.config:sub(1, 1) == "\\"
local sep = windows and "\\" or "/"
local root = os.tmpname()
os.remove(root)
root = root .. " metadata tests"
local function quote(value) return '"' .. value .. '"' end
local function command(args)
  local cmd = table.concat(args, " ")
  return windows and ('"' .. cmd .. '"') or cmd
end
local function run(args)
  local result = os.execute(command(args))
  assert(result == 0 or result == true, "command failed: " .. table.concat(args, " "))
end
local function read(args)
  local pipe = assert(io.popen(command(args)))
  local output = pipe:read("*a")
  pipe:close()
  return output
end
local function exists(path)
  local file = io.open(path, "rb")
  if not file then return false end
  file:close()
  return true
end
local function mkdir(path)
  if windows then
    run({ "if", "not", "exist", quote(path), "mkdir", quote(path) })
  else
    run({ "mkdir", "-p", quote(path) })
  end
end
local function game_tag(path)
  return read({ "ffprobe", "-v", "error", "-show_entries", "format_tags=game",
    "-of", "default=noprint_wrappers=1:nokey=1", quote(path) }):gsub("%s+$", "")
end
local function packets(path)
  return read({ "ffprobe", "-v", "error", "-show_packets", "-show_data_hash", "sha256",
    "-show_entries", "packet=stream_index,data_hash", "-of", "csv=p=0", quote(path) })
end

mkdir(root)
local fixture = root .. sep .. "fixture.mkv"
run({ "ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "color=size=64x64:rate=5",
  "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "0.4",
  "-map", "0:v", "-map", "1:a", "-map", "1:a", "-c:v", "libaom-av1", "-cpu-used", "8",
  "-c:a", "aac", quote(fixture) })
local original_packets = packets(fixture)
assert(original_packets ~= "", "fixture must contain packets")
assert(read({ "ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
  "stream=codec_name", "-of", "default=noprint_wrappers=1:nokey=1", quote(fixture) }):match("av1"),
  "fixture must be AV1")

-- Disable foreground-window detection so the real no-game branch is deterministic.
package.preload.ffi = function() error("foreground detection disabled for tests") end
local callback, current_path, current_title, logs
obslua = {
  LOG_INFO = 1, LOG_WARNING = 2,
  OBS_FRONTEND_EVENT_RECORDING_STOPPED = 1,
  OBS_FRONTEND_EVENT_REPLAY_BUFFER_SAVED = 2,
  obs_frontend_get_current_scene = function() return current_title end,
  obs_scene_from_source = function(source) return source end,
  obs_scene_enum_items = function() return { {} } end,
  obs_sceneitem_visible = function() return true end,
  obs_sceneitem_get_source = function() return {} end,
  obs_source_get_unversioned_id = function() return "window_capture" end,
  obs_source_get_settings = function() return { window = current_title .. ":class:game.exe" } end,
  obs_source_get_name = function() return "Test capture" end,
  obs_data_get_string = function(settings, key) return settings[key] or "" end,
  obs_data_get_bool = function(settings, key) return settings[key] or false end,
  obs_data_release = function() end,
  obs_source_release = function() end,
  sceneitem_list_release = function() end,
  obs_frontend_get_last_replay = function() return current_path end,
  obs_frontend_get_last_recording = function() return current_path end,
  obs_frontend_add_event_callback = function(fn) callback = fn end,
  timer_add = function() end,
  os_file_exists = exists,
  os_mkdirs = mkdir,
  os_rename = function(src, dest) return os.rename(src, dest) and 0 or -1 end,
  os_unlink = function(path) return os.remove(path) and 0 or -1 end,
  script_log = function(_, message) logs[#logs + 1] = message end,
}
dofile("packages/obs-script/qlipq-renamer.lua")
script_load({})

local cases = {
  { name = "fallback replay", game = "Any Recording" },
  { name = "custom fallback", fallback = "Desktop Capture", game = "Desktop Capture" },
  { name = "detected game", title = "Overwatch", game = "Overwatch" },
  { name = "empty sanitized title", title = "!!!", game = "Any Recording" },
  { name = "metadata only", game = "Any Recording", in_place = true },
  { name = "metadata only with OBS path separators", game = "Any Recording", in_place = true, forward_slashes = true },
  { name = "fallback recording", game = "Any Recording", recording = true },
  { name = "failed ffmpeg", game = "Any Recording", fail = true },
}
local failures = 0
for index, case in ipairs(cases) do
  local directory = root .. sep .. index
  mkdir(directory)
  local filename = "2026-10-04 05-08-28 Replay.mkv"
  current_path = directory .. sep .. filename
  local input = assert(io.open(fixture, "rb"))
  local output = assert(io.open(current_path, "wb"))
  output:write(input:read("*a"))
  input:close()
  output:close()
  if case.forward_slashes then current_path = current_path:gsub("\\", "/") end
  current_title, logs = case.title, {}
  script_update({
    fallback_name = case.fallback or "Any Recording", organization_mode = "basic",
    move_to_folders = not case.in_place, write_metadata = true,
    ffmpeg_path = case.fail and "qlipq-missing-ffmpeg" or "ffmpeg", organize_replays = true,
  })
  callback(case.recording and obslua.OBS_FRONTEND_EVENT_RECORDING_STOPPED
    or obslua.OBS_FRONTEND_EVENT_REPLAY_BUFFER_SAVED)
  local dest_dir = case.in_place and directory or (directory .. sep .. case.game)
  local dest = dest_dir .. sep .. filename
  local ok, err = pcall(function()
    assert(exists(dest), "destination missing")
    assert(game_tag(dest) == (case.fail and "" or case.game), "wrong/missing game metadata")
    assert(packets(dest) == original_packets, "video/audio packets changed")
    assert(case.in_place or not exists(current_path), "source was not moved")
    assert(not exists(dest .. ".qqbak"), "backup left behind")
    assert(not exists(dest .. ".qqtmp"), "temporary file left behind")
    assert(not exists(dest:gsub("(%.[^./\\]+)$", ".qqtmp%1")), "temporary file left behind")
    local messages = table.concat(logs, "\n")
    assert(messages:find(case.fail and "moved (untagged)" or
      (case.in_place and "tagged in place" or "tagged + moved"), 1, true), messages)
  end)
  if ok then
    print("PASS: " .. case.name .. " (AV1 + two AAC tracks)")
  else
    failures = failures + 1
    print("FAIL: " .. case.name .. ": " .. err .. "\n" .. table.concat(logs, "\n"))
  end
  os.remove(dest)
  os.remove(current_path)
  os.remove(dest .. ".qqtmp")
  os.remove(dest:gsub("(%.[^./\\]+)$", ".qqtmp%1"))
  os.remove(dest .. ".qqbak")
  if not case.in_place then run({ "rmdir", quote(dest_dir) }) end
  run({ "rmdir", quote(directory) })
end
os.remove(fixture)
run({ "rmdir", quote(root) })
assert(failures == 0, failures .. " metadata test(s) failed")
