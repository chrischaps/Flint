// ─── zone_door.rhai ─────────────────────────────────────
// Zone entrance doors. Reads zone_data component to determine
// target scene and checks persist store for unlock state.
// On interact: loads target scene if unlocked, plays locked sound if not.

let target_scene = "";
let zone_name = "";
let unlock_key = "";

fn on_init() {
    let me = self_entity();
    let tgt = get_field(me, "zone_data", "target_scene");
    if tgt != () {
        target_scene = tgt;
    }
    let zn = get_field(me, "zone_data", "zone_name");
    if zn != () {
        zone_name = zn;
    }

    // Derive unlock key from zone name
    if zone_name == "Sunken Crypt" {
        unlock_key = "zone_crypt_unlocked";
    } else if zone_name == "Flooded Garden" {
        unlock_key = "zone_garden_unlocked";
    } else if zone_name == "Collapsed Library" {
        unlock_key = "zone_library_unlocked";
    } else if zone_name == "Tavern" {
        unlock_key = "always_open";
    }

    // A door is not a warning: `log` is info, which the player shows (#716)
    log("[zone_door] ready: zone=" + zone_name + " target=" + target_scene + " key=" + unlock_key);
}

fn on_update() {
    let me = self_entity();
    if unlock_key == "always_open" {
        set_field(me, "door", "locked", false);
        return;
    }
    let unlocked = persist_get(unlock_key);
    if unlocked == true {
        set_field(me, "door", "locked", false);
    } else {
        set_field(me, "door", "locked", true);
    }
}

fn on_interact() {
    let me = self_entity();
    let locked = get_field(me, "door", "locked");
    if locked == true {
        play_sound("audio/door_locked.wav");
        log("[zone_door] LOCKED — need more essence for " + zone_name);
        return;
    }

    if target_scene != "" {
        play_sound("audio/door_open.ogg");
        log("[zone_door] ENTERING " + zone_name + " via " + target_scene);
        load_scene(target_scene);
    } else {
        log_warn("[zone_door] ERROR: target_scene is empty!");
    }
}
