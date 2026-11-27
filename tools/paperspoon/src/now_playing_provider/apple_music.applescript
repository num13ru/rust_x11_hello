on run argv
    if application id "com.apple.Music" is not running then return "unavailable"
    set operation to item 1 of argv
    tell application id "com.apple.Music"
        set stateText to player state as text
        if stateText is "stopped" then return "stopped"
        set currentTrack to current track
        set trackKey to my trackIdentity(currentTrack)
        if operation is "sample" then
            set trackTitle to ""
            set trackArtist to ""
            set trackAlbum to ""
            set trackPosition to ""
            set trackDuration to ""
            set shuffleText to ""
            set repeatText to ""
            try
                set trackTitle to my cleanField(name of currentTrack)
            end try
            try
                set trackArtist to my cleanField(artist of currentTrack)
            end try
            try
                set trackAlbum to my cleanField(album of currentTrack)
            end try
            try
                set trackPosition to player position as text
            end try
            try
                set trackDuration to duration of currentTrack as text
            end try
            try
                if shuffle enabled then
                    set shuffleText to "on"
                else
                    set shuffleText to "off"
                end if
            end try
            try
                set repeatSetting to song repeat
                if repeatSetting is off then
                    set repeatText to "off"
                else if repeatSetting is all then
                    set repeatText to "all"
                else if repeatSetting is one then
                    set repeatText to "one"
                end if
            end try
            set AppleScript's text item delimiters to character id 31
            return {stateText, trackKey, trackTitle, trackArtist, trackAlbum, trackPosition, trackDuration, shuffleText, repeatText} as text
        end if

        if trackKey is "" or trackKey is not item 2 of argv then return "track changed"
        if operation is "artwork" then
            if (count artworks of currentTrack) is 0 then return "missing"
            set artworkData to raw data of artwork 1 of currentTrack
            if my trackIdentity(current track) is not trackKey then return "track changed"
            set outputPath to item 3 of argv
        else if operation is "reveal" then
            reopen
            activate
            if (count of browser windows) > 0 then
                set collapsed of browser window 1 to false
                set visible of browser window 1 to true
            end if
            -- Some streamed tracks cannot be revealed in the library.
            try
                if my trackIdentity(current track) is trackKey then
                    reveal current track
                    return "revealed"
                end if
            end try
        else if operation is "previous" then
            previous track
        else if operation is "playpause" then
            playpause
        else if operation is "next" then
            next track
        else if operation is "shuffle" then
            set shuffle enabled to (not (get shuffle enabled))
        else if operation is "repeat" then
            set repeatSetting to song repeat
            if repeatSetting is off then
                set song repeat to all
            else if repeatSetting is all then
                set song repeat to one
            else if repeatSetting is one then
                set song repeat to off
            else
                return "repeat mode unavailable"
            end if
        else if operation is "seek" then
            set positionSeconds to ((item 3 of argv) as real) / 1000
            set trackDuration to duration of currentTrack
            if trackDuration is missing value or trackDuration is less than or equal to 0 then return "duration unavailable"
            if positionSeconds is greater than trackDuration then set positionSeconds to trackDuration
            if my trackIdentity(current track) is not trackKey then return "track changed"
            set player position to positionSeconds
        else
            return "unknown operation"
        end if
    end tell
    if operation is "artwork" then
        set artworkFile to open for access (POSIX file outputPath) with write permission
        try
            write artworkData to artworkFile
        on error errorMessage number errorNumber
            close access artworkFile
            error errorMessage number errorNumber
        end try
        close access artworkFile
    end if
    return "ok"
end run

on trackIdentity(theTrack)
    tell application id "com.apple.Music"
        try
            set identifier to persistent ID of theTrack
            if identifier is not missing value and identifier is not "" then return "pid:" & identifier
        end try
        try
            set identifier to id of theTrack
            if identifier is greater than 0 then return "id:" & (identifier as text)
        end try
    end tell
    return ""
end trackIdentity

on cleanField(value)
    if value is missing value then return ""
    set AppleScript's text item delimiters to character id 31
    set fields to text items of (value as text)
    set AppleScript's text item delimiters to " "
    return fields as text
end cleanField
