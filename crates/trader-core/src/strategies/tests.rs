use super::*;

/// Every combination of the four signals, from each target: an entry beats an exit, an exit alone
/// goes flat, two entries at once are no signal, and a held target stays.
#[test]
fn next_target_table() {
    use Target::*;
    let signals =
        |enter_long, enter_short, exit_long, exit_short| Signals { enter_long, enter_short, exit_long, exit_short };
    let none = signals(false, false, false, false);
    // From flat.
    assert_eq!(next_target(Flat, none), Flat);
    assert_eq!(next_target(Flat, signals(true, false, false, false)), Long);
    assert_eq!(next_target(Flat, signals(false, true, false, false)), Short);
    assert_eq!(next_target(Flat, signals(true, true, false, false)), Flat);
    // Exits do nothing from flat.
    assert_eq!(next_target(Flat, signals(false, false, true, true)), Flat);
    // From long: an opposite entry reverses even when the exit also fires, an exit goes flat.
    assert_eq!(next_target(Long, none), Long);
    assert_eq!(next_target(Long, signals(false, true, true, false)), Short);
    assert_eq!(next_target(Long, signals(false, false, true, false)), Flat);
    assert_eq!(next_target(Long, signals(true, false, false, true)), Long);
    // From short, the mirror image.
    assert_eq!(next_target(Short, none), Short);
    assert_eq!(next_target(Short, signals(true, false, false, true)), Long);
    assert_eq!(next_target(Short, signals(false, false, false, true)), Flat);
    assert_eq!(next_target(Short, signals(false, true, true, false)), Short);
}
