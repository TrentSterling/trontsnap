use super::{tray_click_decision, TrayClickDecision};

#[test]
fn first_tray_click_defers_capture() {
    assert_eq!(
        tray_click_decision(10_000, 0, 0),
        TrayClickDecision::DeferCapture
    );
}

#[test]
fn quick_second_tray_click_restores_without_native_double_click_event() {
    assert_eq!(
        tray_click_decision(10_120, 0, 10_000),
        TrayClickDecision::Restore
    );
}

#[test]
fn native_double_click_trailing_up_is_consumed() {
    assert_eq!(
        tray_click_decision(10_140, 10_120, 10_000),
        TrayClickDecision::Consume
    );
}

#[test]
fn later_tray_click_stays_a_single_capture() {
    assert_eq!(
        tray_click_decision(11_000, 10_120, 10_000),
        TrayClickDecision::DeferCapture
    );
}
