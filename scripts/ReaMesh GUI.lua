-- ReaMesh GUI: host and join LAN collaboration sessions.
--
-- This file is installed and kept up to date by the ReaMesh extension
-- (reaper_reamesh); manual edits are overwritten when REAPER starts.
-- Requires ReaImGui 0.9+ -- install it via Extensions > ReaPack > Browse
-- packages, then restart REAPER.

if not reaper.ReaMesh_GetStatus then
  reaper.MB('The ReaMesh extension is not loaded (reaper_reamesh).', 'ReaMesh', 0)
  return
end
if not reaper.ImGui_GetBuiltinPath then
  reaper.MB('ReaMesh GUI needs the ReaImGui extension (0.9 or newer).\n\n'
    .. 'Install it via Extensions > ReaPack > Browse packages,\n'
    .. 'then restart REAPER and run this action again.', 'ReaMesh', 0)
  return
end

-- Relaunching the action terminates the previous instance instead of erroring.
if reaper.set_action_options then reaper.set_action_options(1) end

package.path = reaper.ImGui_GetBuiltinPath() .. '/?.lua'
local ImGui = require 'imgui' '0.9'

local ctx = ImGui.CreateContext('ReaMesh')

-- Passphrase inputs persist across frames: one for hosting, one per session id.
local host_pass = ''
local join_pass = {}

local function get_status()
  local st = {}
  for line in reaper.ReaMesh_GetStatus():gmatch('[^\n]+') do
    local k, v = line:match('^([^=]+)=(.*)$')
    if k then st[k] = v end
  end
  return st
end

local function get_sessions()
  local list = {}
  for line in reaper.ReaMesh_GetSessions():gmatch('[^\n]+') do
    local id, name, file, auth = line:match('^([^\t]*)\t([^\t]*)\t([^\t]*)\t(.*)$')
    if id and id ~= '' then
      list[#list + 1] = { id = id, name = name, file = file, auth = auth == '1' }
    end
  end
  return list
end

local function dots()
  return string.rep('.', math.floor(ImGui.GetTime(ctx) * 2) % 4)
end

local function draw_browser(st)
  -- Discovery starts as soon as the panel is open.
  if st.mode == 'idle' then reaper.ReaMesh_StartBrowse() end

  ImGui.Text(ctx, 'Sessions on this network')
  ImGui.Separator(ctx)
  local list = get_sessions()
  if #list == 0 then
    ImGui.TextDisabled(ctx, 'searching' .. dots())
  end
  for _, s in ipairs(list) do
    if ImGui.Button(ctx, 'Join##' .. s.id) then
      reaper.ReaMesh_JoinSession(s.id, join_pass[s.id] or '')
    end
    ImGui.SameLine(ctx)
    ImGui.Text(ctx, (s.auth and '[lock] ' or '') .. s.name)
    ImGui.SameLine(ctx)
    ImGui.TextDisabled(ctx, '(' .. s.file .. ')')
    -- Protected sessions need the passphrase typed before Join works.
    if s.auth then
      ImGui.SetNextItemWidth(ctx, 180)
      local changed, val = ImGui.InputText(ctx, 'passphrase##' .. s.id,
        join_pass[s.id] or '', ImGui.InputTextFlags_Password)
      if changed then join_pass[s.id] = val end
    end
  end

  ImGui.Spacing(ctx)
  ImGui.Spacing(ctx)
  ImGui.Text(ctx, 'Share this project')
  ImGui.Separator(ctx)
  ImGui.SetNextItemWidth(ctx, 180)
  local pchanged, pval = ImGui.InputText(ctx, 'Passphrase (optional)', host_pass,
    ImGui.InputTextFlags_Password)
  if pchanged then host_pass = pval end
  if ImGui.Button(ctx, 'Host current project') then
    reaper.ReaMesh_Host(host_pass)
  end
  ImGui.TextWrapped(ctx, 'Shares every file in the project folder with people who join. '
    .. 'Media outside the project folder is copied in automatically. '
    .. 'Set a passphrase to limit who can download and edit.')
end

local function draw_downloading(st)
  ImGui.Text(ctx, 'Joining "' .. (st.session or '?') .. '"')
  ImGui.Spacing(ctx)
  ImGui.ProgressBar(ctx, tonumber(st.progress) or 0)
  ImGui.TextDisabled(ctx, 'downloading the project' .. dots())
  ImGui.Spacing(ctx)
  if ImGui.Button(ctx, 'Cancel') then
    reaper.ReaMesh_Leave()
  end
end

local function draw_session(st)
  if st.mode == 'hosting' then
    ImGui.Text(ctx, 'Hosting "' .. (st.session or '?') .. '"')
    ImGui.TextDisabled(ctx, 'Your edits win conflicts.')
  else
    ImGui.Text(ctx, 'In session "' .. (st.session or '?') .. '"')
    ImGui.TextDisabled(ctx, 'The host wins edit conflicts.')
  end
  ImGui.Spacing(ctx)
  local peers = tonumber(st.peers) or 0
  if peers > 0 then
    ImGui.TextColored(ctx, 0x4CD964FF,
      ('%d peer%s connected'):format(peers, peers == 1 and '' or 's'))
  else
    ImGui.TextDisabled(ctx, 'waiting for peers' .. dots())
  end
  ImGui.Spacing(ctx)
  if ImGui.Button(ctx, st.mode == 'hosting' and 'Stop hosting' or 'Leave session') then
    reaper.ReaMesh_Leave()
  end
end

local function draw_log()
  ImGui.Spacing(ctx)
  if not ImGui.CollapsingHeader(ctx, 'Activity') then return end
  local log = reaper.ReaMesh_GetLog()
  if log == '' then
    ImGui.TextDisabled(ctx, 'nothing yet')
    return
  end
  if ImGui.BeginChild(ctx, 'activity_log', 0, 120) then
    for line in log:gmatch('[^\n]+') do
      ImGui.TextWrapped(ctx, line)
    end
    -- Keep the newest entry in view.
    if ImGui.GetScrollY(ctx) >= ImGui.GetScrollMaxY(ctx) - 1 then
      ImGui.SetScrollHereY(ctx, 1.0)
    end
    ImGui.EndChild(ctx)
  end
end

local function draw()
  local st = get_status()
  if st.error and st.error ~= '' then
    ImGui.TextColored(ctx, 0xFF5050FF, st.error)
    ImGui.Separator(ctx)
    ImGui.Spacing(ctx)
  end
  if st.mode == 'idle' or st.mode == 'browsing' then
    draw_browser(st)
  elseif st.mode == 'downloading' then
    draw_downloading(st)
  elseif st.mode == 'hosting' or st.mode == 'client' then
    draw_session(st)
  end
  draw_log()
end

local function loop()
  ImGui.SetNextWindowSize(ctx, 400, 300, ImGui.Cond_FirstUseEver)
  local visible, open = ImGui.Begin(ctx, 'ReaMesh', true)
  if visible then
    local ok, err = pcall(draw)
    ImGui.End(ctx)
    if not ok then error(err, 0) end
  end
  if open then
    reaper.defer(loop)
  else
    -- Closing the panel stops discovery, but never tears down a live session.
    local st = get_status()
    if st.mode == 'browsing' then reaper.ReaMesh_Leave() end
  end
end

reaper.defer(loop)
