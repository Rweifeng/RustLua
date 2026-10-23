-- generated from assets/graphs/p3-e03-interact.vgraph; do not edit
-- graph: p3-e03-interact (vgraph 1)

local M = {}

function M.on_start(owner)
    -- node seq-start: sequence
    ::flow_2::
    -- node move: set_transform
    ::flow_4::
    do
        local n_num_x = 1.5
        local n_num_a = 2
        local n_num_b = 3
        local n_math_y = (n_num_a + n_num_b)
        local n_num_z = 0
        transform3d.set(owner, n_num_x, n_math_y, n_num_z)
    end
    goto cont_3
    ::cont_3::
    -- node save-start: save_game
    ::flow_5::
    do
        save.commit()
    end
    goto fend_1
    ::fend_1::
    -- on_start.
end

function M.on_update(owner, dt)
    -- node gate: gate
    ::flow_7::
    do
        local n_flag = true
        if not (n_flag) then goto fend_6 end
    end
    -- node tone: play_audio
    ::flow_8::
    do
        audio.play("tick")
    end
    goto fend_6
    ::fend_6::
    -- on_update.
end

function M.on_interact(owner, target)
    -- node seq-interact: sequence
    ::flow_10::
    -- node anim: play_animation
    ::flow_12::
    do
        animation3d.set(owner, "spin")
    end
    goto cont_11
    ::cont_11::
    -- node check: branch
    ::flow_13::
    do
        local n_flag2 = true
        if n_flag2 then
            -- node say: ui_set_text
            ::flow_14::
            do
                local n_msg = "hello \"world\"\n"
                ui.text("hint", n_msg)
            end
            goto fend_9
        else
            -- node nope: emit_event
            ::flow_15::
            do
                event.emit("no_anim")
            end
            goto fend_9
        end
    end
    ::fend_9::
    -- on_interact.
end

function M.on_collision(owner, other)
    -- node seq-hit: sequence
    ::flow_17::
    -- node hit: emit_event
    ::flow_19::
    do
        event.emit("hit")
    end
    goto cont_18
    ::cont_18::
    -- node save-hit: save_game
    ::flow_20::
    do
        save.commit()
    end
    goto fend_16
    ::fend_16::
    -- on_collision.
end

return M
