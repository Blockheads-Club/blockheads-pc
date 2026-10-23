local function at(p) return p.x .. "," .. p.y end
local function who(p) return p and p:alias() or "-" end
return {
    on_enable = function()
        bh.events.on("world-save", function(ev) bh.log.info("event world-save " .. ev.phase) end)
        bh.events.on("block-place", function(ev)
            bh.log.info("event block-place " .. who(ev.player) .. " " .. at(ev.pos) .. " " .. ev.item)
        end)
        bh.events.on("block-break", function(ev)
            bh.log.info("event block-break " .. who(ev.player) .. " " .. at(ev.pos) .. " " .. ev.layer .. " " .. ev.tile)
        end)
        bh.events.on("object-place", function(ev)
            bh.log.info("event object-place " .. who(ev.player) .. " " .. ev.object .. " " .. ev.item .. " " .. at(ev.pos))
        end)
        bh.events.on("damage", function(ev) bh.log.info("event damage " .. who(ev.player) .. " " .. ev.amount) end)
        bh.events.on("death", function(ev) bh.log.info("event death " .. who(ev.player)) end)
        bh.events.on("player-move", function(ev)
            bh.log.info("event player-move " .. ev.player:alias() .. " " .. at(ev.from) .. " " .. at(ev.to))
        end)
        bh.commands.register({ name = "where", usage = "/where" }, function(sender)
            local p = sender.player:pos()
            bh.log.info("where: " .. (p and at(p) or "nowhere"))
            return { handled = true, reply = p and at(p) or "nowhere" }
        end)
        bh.log.info("watch enabled")
    end,
}
