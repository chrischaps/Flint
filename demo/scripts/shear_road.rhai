// Lays the scanline road every frame (#625): each tile is pushed sideways
// by its distance (x += dx; dx += curve) and sheared so its near edge meets
// the last tile's far edge. One entity and two calls a tile; the children
// (road, curbs, lane line) follow through their parent's matrix.
//
// At time 0 this is exactly the road demo/tools/shear_road.py wrote; then
// the bend breathes from right to left and back. 22 tiles of 8 m (Rhai
// functions can't see top-level constants, so the numbers are inline).

fn lay(ids, t) {
    let sway = (t * 0.6).cos();
    let x = 0.0;
    let dx = 0.0;
    let y = 0.0;
    let dy = 0.0;
    for i in 0..22 {
        let curve = if i < 9 { 0.34 } else if i < 18 { -0.42 } else { 0.0 };
        let hill = if i >= 4 && i < 11 { 0.06 } else if i >= 11 && i < 17 { -0.07 } else { 0.0 };
        dx += curve * sway;
        dy += hill;
        let x1 = x + dx;
        let y1 = y + dy;
        let id = ids[i];
        set_position(id, (x + x1) / 2.0, (y + y1) / 2.0 - 0.05, -(i.to_float() + 0.5) * 8.0);
        set_shear(id, #{ xz: -(x1 - x) / 8.0, yz: -(y1 - y) / 8.0 });
        x = x1;
        y = y1;
    }
}

fn on_update() {
    let ids = [];
    for i in 0..22 {
        let n = if i < 10 { `tile_0${i}` } else { `tile_${i}` };
        ids.push(get_entity(n));
    }
    lay(ids, total_time());
}
