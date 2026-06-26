use crate::harness::state::RecoveryState;

pub fn check_no_progress(
    state: &mut RecoveryState,
    had_file_read: bool,
    had_successful_edit: bool,
    had_successful_test: bool,
) -> bool {
    if had_file_read || had_successful_edit || had_successful_test {
        state.consecutive_no_progress = 0;
        return false;
    }
    state.consecutive_no_progress += 1;
    state.consecutive_no_progress >= 5
}
