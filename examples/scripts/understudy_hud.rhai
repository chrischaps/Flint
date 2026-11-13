// Understudy (#407): the page the film is printed on. A comic panel's
// border and gutter round the frame, caption boxes that come and go with
// the director's beats (understudy.rhai, the same 28-second clock), and
// the tear's sound lettered over it. All of it draw calls: no engine code.
//
// Captions and the border are redrawn on twos, like the pencil and the
// figure inside the drawing: every corner moves a hair on each of the
// twelve ticks a second, so the page is drawn by the same hand.

// The captions: when each shows, its corner (0 top left, 1 bottom right)
// and its words. Lettered in capitals, as captions are.
fn captions() {
    [
        [0.4, 3.4, 0, "EVERY EVENING THE CLEARING SAT FOR ITS PORTRAIT."],
        [1.5, 3.4, 1, "THE PAINTER WAS LATE AGAIN."],
        [3.6, 6.4, 0, "SO THE MANNEQUIN WENT TO SEE WHAT THE PAGE WAS WAITING FOR."],
        [10.4, 14.2, 0, "ON THE PAGE, NOTHING WAS FINISHED."],
        [12.2, 17.6, 1, "EVERY LINE WAS STILL DECIDING WHAT IT WOULD BE."],
        [20.6, 23.4, 0, "IT WAS BACK BEFORE THE PAINTER CAME."],
        [23.8, 27.8, 1, "MOSTLY."],
    ]
}

fn font() { "PatrickHand-Regular" }

// A small, fixed wobble for corner k on tick n: the same every run
fn jit(n, k) {
    let h = sin(n * 12.9898 + k * 78.233) * 43758.5453;
    (h - floor(h)) * 2.0 - 1.0
}

// A box with a hand-drawn outline: four corners nudged per tick
fn sketch_box(x, y, w, h, tick, seed, fill, ink, layer) {
    let j = 1.4;
    let pts = [
        [x + j * jit(tick, seed + 1.0), y + j * jit(tick, seed + 2.0)],
        [x + w + j * jit(tick, seed + 3.0), y + j * jit(tick, seed + 4.0)],
        [x + w + j * jit(tick, seed + 5.0), y + h + j * jit(tick, seed + 6.0)],
        [x + j * jit(tick, seed + 7.0), y + h + j * jit(tick, seed + 8.0)],
    ];
    draw_polygon(pts, #{ color: fill, layer: layer });
    draw_polygon_outline(pts, #{ color: ink, thickness: 2.2, closed: true, layer: layer + 1 });
}

// Words broken into lines no wider than `wide` points
fn wrap(text, size, wide) {
    let lines = [];
    let line = "";
    for word in text.split(" ") {
        let next = if line == "" { word } else { line + " " + word };
        if line != "" && measure_text(next, size, #{ font: font() }).width > wide {
            lines.push(line);
            line = word;
        } else {
            line = next;
        }
    }
    if line != "" { lines.push(line); }
    lines
}

fn caption(text, corner, alpha, tick, seed, sw, sh, gutter) {
    let size = 27.0;
    let pad = 13.0;
    let lines = wrap(text, size, min(sw * 0.48, 620.0));
    let wide = 0.0;
    for l in lines { wide = max(wide, measure_text(l, size, #{ font: font() }).width); }
    let lh = size * 1.12;
    let w = wide + pad * 2.0;
    let h = lh * lines.len().to_float() + pad * 2.0 - 4.0;
    // Captions sit on the panel's edge, as a printed one does
    let x = if corner == 0 { gutter + 14.0 } else { sw - gutter - 14.0 - w };
    let y = if corner == 0 { gutter - 8.0 } else { sh - gutter - h + 8.0 };
    // In: drops 6 points as it fades up
    y += (1.0 - alpha) * (if corner == 0 { -6.0 } else { 6.0 });
    sketch_box(x, y, w, h, tick, seed, [1.0, 0.95, 0.80, alpha], [0.09, 0.08, 0.07, alpha], 20);
    let ty = y + pad - 2.0;
    for l in lines {
        draw_text(x + pad, ty, l, size, #{ font: font(), color: [0.09, 0.08, 0.07, alpha], layer: 22 });
        ty += lh;
    }
}

fn on_draw_ui() {
    let t = total_time() % 28.0;
    let tick = floor(total_time() * 12.0);
    let sw = screen_width();
    let sh = screen_height();
    let gutter = 18.0;
    let paper = [0.97, 0.96, 0.93, 1.0];
    let ink = [0.09, 0.08, 0.07, 1.0];

    // The gutter: the white of the page round the panel
    draw_rect(0.0, 0.0, sw, gutter, #{ color: paper, layer: 10 });
    draw_rect(0.0, sh - gutter, sw, gutter, #{ color: paper, layer: 10 });
    draw_rect(0.0, 0.0, gutter, sh, #{ color: paper, layer: 10 });
    draw_rect(sw - gutter, 0.0, gutter, sh, #{ color: paper, layer: 10 });
    // The panel's border, inked, drawn again each tick
    let j = 0.9;
    let corners = [
        [gutter + j * jit(tick, 11.0), gutter + j * jit(tick, 12.0)],
        [sw - gutter + j * jit(tick, 13.0), gutter + j * jit(tick, 14.0)],
        [sw - gutter + j * jit(tick, 15.0), sh - gutter + j * jit(tick, 16.0)],
        [gutter + j * jit(tick, 17.0), sh - gutter + j * jit(tick, 18.0)],
    ];
    draw_polygon_outline(corners, #{ color: ink, thickness: 3.2, closed: true, layer: 11 });
    // The title, small, in the bottom gutter, clear of the captions
    draw_text(gutter, sh - gutter + 1.0, "UNDERSTUDY", 13.0,
        #{ font: font(), color: [0.09, 0.08, 0.07, 0.55], layer: 12 });

    // The captions in their time
    let all = captions();
    for i in 0..all.len() {
        let c = all[i];
        if t >= c[0] && t < c[1] {
            let alpha = min(clamp((t - c[0]) / 0.25, 0.0, 1.0), clamp((c[1] - t) / 0.25, 0.0, 1.0));
            caption(c[3], c[2], alpha, tick, i.to_float() * 10.0, sw, sh, gutter);
        }
    }

    // The tear's sound, lettered where it starts: the page
    if t >= 5.95 && t < 7.3 {
        let page = get_world_position(get_entity("page"));
        let s = world_to_screen(page.x, page.y, page.z);
        let grow = clamp((t - 5.95) / 0.2, 0.0, 1.0);
        let fade = clamp((7.3 - t) / 0.3, 0.0, 1.0);
        let size = 40.0 + 44.0 * grow;
        let dx = 3.0 * jit(tick, 41.0);
        let dy = 3.0 * jit(tick, 42.0);
        draw_text(s.x + 30.0 + dx, s.y - 170.0 + dy, "RRRIP!", size, #{
            font: font(),
            color: [1.0, 0.86, 0.26, fade],
            stroke: [0.09, 0.08, 0.07, fade, 4.0],
            align: "center",
            layer: 30,
        });
    }
}
