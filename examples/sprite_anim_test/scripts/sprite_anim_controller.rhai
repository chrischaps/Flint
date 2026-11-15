// Sprite animation test controller
// Cycles through animation clips to validate WP2 sprite sheet animation

let current_clip = "idle";
let timer = 0.0;
let cycle_index = 0;

fn on_init() {
    let me = self_entity();
    play_sprite(me, "idle");
    log("[sprite_anim] Started idle animation");
}

fn on_update() {
    let dt = delta_time();
    timer += dt;

    let me = self_entity();

    // Cycle through clips every 3 seconds
    if timer >= 3.0 {
        timer = 0.0;
        cycle_index += 1;

        if cycle_index % 4 == 1 {
            play_sprite(me, "run");
            set_sprite_speed(me, 1.0);
            current_clip = "run";
            log("[sprite_anim] Switched to run (loop)");
        } else if cycle_index % 4 == 2 {
            play_sprite(me, "jump");
            current_clip = "jump";
            log("[sprite_anim] Switched to jump (once)");
        } else if cycle_index % 4 == 3 {
            play_sprite(me, "celebrate");
            current_clip = "celebrate";
            log("[sprite_anim] Switched to celebrate (ping_pong)");
        } else {
            play_sprite(me, "idle");
            current_clip = "idle";
            log("[sprite_anim] Switched to idle (loop)");
        }
    }

    // Test is_sprite_playing
    let playing = is_sprite_playing(me);
    if !playing && current_clip == "jump" {
        log("[sprite_anim] Jump animation ended (playing=false detected)");
    }
}

fn on_animation_end(clip_name) {
    log("[sprite_anim] on_animation_end fired: " + clip_name);
    let me = self_entity();
    // After jump finishes, go back to idle
    if clip_name == "jump" {
        play_sprite(me, "idle");
        current_clip = "idle";
        log("[sprite_anim] Returned to idle after jump");
    }
}
