local ch
return {
    on_enable = function()
        ch = bh.channels.open("echo:ping", function(origin, data)
            bh.log.info("client got " .. data)
            bh.players.broadcast("[echo] " .. data)
        end)
        bh.events.on("chat", function(ev) bh.log.info("chat seen: " .. ev.text) end)
        bh.commands.register({ name = "echo", usage = "/echo <text>" }, function(sender, label, args)
            bh.channels.send(ch, "server", table.concat(args, " "))
            return { handled = true, reply = "sent to the server" }
        end)
        bh.log.info("echo client enabled on " .. bh.host.host_name())
        bh.channels.send(ch, "server", "hello")
    end,
}
