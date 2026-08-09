use super::DepKind;

/// A step-group unit is labelled with the door that grouped it, and the
/// label is that door's constant rather than a second spelling of it —
/// the "three ends, no definition" finding, with the third end removed.
#[test]
fn the_wire_label_of_a_step_group_is_the_door_that_made_it() {
    assert_eq!(
        DepKind::StepGroup(0).wire_name(),
        crate::registration::STEP_GROUP_NAME
    );
    assert_eq!(DepKind::StepGroup(7).wire_name(), "step_group");
    assert_eq!(DepKind::Sequential.wire_name(), "sequential");
}
