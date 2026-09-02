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
    log_warn("[zone_door] on_init: tgt=" + tgt + " type=" + type_of(tgt));
    if tgt != () {
        target_scene = tgt;
    }
    let zn = get_field(me, "zone_data", "zone_name");
    log_warn("[zone_door] on_init: zn=" + zn + " type=" + type_of(zn));
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

    log_warn("[zone_door] ready: zone=" + zone_name + " target=" + target_scene + " key=" + unlock_key);
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
    log_warn("[zone_door] on_interact: locked=" + locked + " type=" + type_of(locked) + " target=" + target_scene + " zone=" + zone_name);
    if locked == true {
        play_sound("audio/door_locked.wav");
        log_warn("[zone_door] LOCKED — need more essence for " + zone_name);
        return;
    }

    if target_scene != "" {
        play_sound("audio/door_open.ogg");
        log_warn("[zone_door] ENTERING " + zone_name + " via " + target_scene);
        load_scene(target_scene);
    } else {
        log_warn("[zone_door] ERROR: target_scene is empty!");
    }
}
