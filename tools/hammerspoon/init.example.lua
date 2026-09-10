--[[
    Hammerspoon config — media keys for the Kindle button grid.

    Chain: Kindle -> paperspoon (TCP 5581) -> `open -g hammerspoon://paperpad/<action>`
           -> hs.urlevent -> music app control.

    paperspoon runs `open -g hammerspoon://paperpad/media.next` once per
    action line it receives. hs.urlevent fires exactly once per URL open, so
    actions dispatch once. Media actions control the Music app via in-process
    AppleScript.

    paperspoon usage:
        tools/paperspoon/target/release/paperspoon 5581 /tmp/paperspoon.log
--]]



-- Existing config: auto-reload on file changes.
function reloadConfig(files)
    doReload = false
    for _, file in pairs(files) do
        if file:sub(-4) == ".lua" then
            doReload = true
        end
    end
    if doReload then
        hs.reload()
    end
end
hs.pathwatcher.new(os.getenv("HOME") .. "/.hammerspoon/", reloadConfig):start()
hs.notify.new({ title = "Hammerspoon", informativeText = "Config loaded" }):send()

-- Headroom proxy health watcher (pre-existing).
local proxyCheckInterval = 10 * 60 -- 10 minutes
local proxyCheckUrl = "http://127.0.0.1:8787/readyz"
local lastProxyUp = nil

local function notifyProxyTransition(up)
    local state = up and "UP" or "DOWN"
    hs.notify
        .new({
            title = "Headroom proxy " .. state,
            informativeText = up and ("Proxy reachable at " .. proxyCheckUrl)
                or ("Proxy unreachable at " .. proxyCheckUrl),
        })
        :send()
end

local function checkProxy()
    hs.http.get(proxyCheckUrl, nil, function(status)
        local up = (status == 200)
        if up ~= lastProxyUp then
            if lastProxyUp ~= nil then
                notifyProxyTransition(up)
            end
            lastProxyUp = up
        end
    end)
end

hs.timer.doEvery(proxyCheckInterval, checkProxy)

-- ==========================================================================
-- PaperPad -> media keys
-- ==========================================================================

-- Existing proxy watcher config ends here; the Kindle media-key listener
-- begins below. Keep this section intact when editing the live config.



-- Media actions drive the Mac's frontmost music app via AppleScript.
-- Direct `hs.eventtap.event.newSystemKeyEvent` posting requires macOS
-- Accessibility permission and silently no-ops without it; AppleScript to
-- Music works without extra entitlements.
local function musicControl(verb)
    -- hs.osascript runs in-process (no shell), avoiding the hung
    -- `osascript` subprocess that `hs.execute` produced.
    hs.osascript.applescript('tell application "Music" to ' .. verb)
end

local ACTION_DISPATCH = {
    ["media.play_pause"] = function()
        musicControl("playpause")
    end,
    ["media.next"] = function()
        musicControl("next track")
    end,
    ["media.previous"] = function()
        musicControl("previous track")
    end,
    ["terminal.new_window"] = function()
        -- Open a new Terminal window via keystroke.
        hs.eventtap.keyStroke({ "cmd" }, "n")
        -- hs.application.launchOrFocus("Terminal")
    end,
    ["tmux.work"] = function()
        -- Placeholder; tmux has no global media key.
        hs.notify.new({ title = "PaperPad", informativeText = "tmux.work" }):send()
    end,
    ["zoom.toggle_mute"] = function()
        -- Zoom global mute: cmd+shift+a.
        hs.eventtap.keyStroke({ "cmd", "shift" }, "a")
    end,
}

local function dispatchAction(actionId)
    local handler = ACTION_DISPATCH[actionId]
    if handler then
        handler()
        return
    end
    hs.notify.new({ title = "PaperPad", informativeText = "Unknown action: " .. actionId }):send()
end



-- React to `hammerspoon://paperpad?action=<id>` URL events from paperspoon.
-- paperspoon runs `open -g hammerspoon://paperpad?action=media.next` per action.
-- hs.urlevent fires exactly once per URL open; native, no sockets, no polling.
hs.urlevent.bind("paperpad", function(eventName, params)
    local actionId = params and params.action or ""
    if actionId ~= "" then
        dispatchAction(actionId)
    end
end)
