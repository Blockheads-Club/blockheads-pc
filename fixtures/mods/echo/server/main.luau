local ch
return {
    on_enable = function()
        ch = bh.channels.open("echo:ping", function(origin, data)
            bh.log.info("server got " .. data)
            bh.channels.send(ch, origin.player, "pong:" .. data)
            bh.players.broadcast("echo heard " .. data)
        end)
    end,
}
