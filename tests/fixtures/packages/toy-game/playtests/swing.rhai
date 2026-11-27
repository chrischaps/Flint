// The toy package's playtest (#1560): the spring is pulled inside each fixed
// step (`toy_steps()` counts them), so bob swings through the origin and
// back, held within a metre of it.
let lowest = 2.0;

fn on_update() {
    let x = get_position(get_entity("bob")).x;
    test_assert(x <= 1.001 && x >= -1.001, "the spring holds bob within a metre");
    if x < lowest {
        lowest = x;
    }
    test_log("springs", toy_springs());
    test_log("steps", toy_steps());
    if toy_steps() >= 120 {
        test_assert(lowest < -0.6, "bob swung through the origin");
        test_log("lowest", lowest);
        test_pass();
    }
}
