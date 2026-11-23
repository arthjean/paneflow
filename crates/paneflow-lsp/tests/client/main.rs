#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fake_server;
mod lifecycle;
mod support;
mod sync;
mod tokens;

use libtest_mimic::{Arguments, Trial};

macro_rules! trials {
    ($($module:ident :: $name:ident),* $(,)?) => {
        vec![$(Trial::test(concat!(stringify!($module), "::", stringify!($name)), || {
            $module::$name();
            Ok(())
        })),*]
    };
}

fn main() {
    if let Ok(scenario) = std::env::var(fake_server::SCENARIO_ENV) {
        fake_server::run(&scenario);
        return;
    }
    let trials = trials![
        lifecycle::initialize_carries_the_root_and_the_capabilities_then_initialized_precedes_documents,
        lifecycle::a_second_document_of_the_same_server_and_root_starts_no_second_process,
        lifecycle::closing_the_last_document_sends_shutdown_then_exit,
        lifecycle::a_server_still_alive_after_exit_is_killed,
        lifecycle::quitting_sends_shutdown_and_exit_to_every_running_server,
        lifecycle::a_first_unexpected_exit_restarts_once_and_a_second_leaves_it_crashed,
        lifecycle::a_first_protocol_error_restarts_once_and_a_second_leaves_it_crashed,
        lifecycle::an_initialize_error_stops_the_server_in_failed_with_its_message,
        lifecycle::a_missing_command_enters_the_missing_state_and_starts_nothing,
        lifecycle::workspace_configuration_is_answered_from_the_entry_settings,
        lifecycle::a_reported_missing_server_is_in_the_missing_state_without_a_process,
        sync::did_open_sends_the_uri_the_language_id_version_one_and_the_full_text,
        sync::an_edit_batch_sends_one_incremental_did_change_in_utf16_counting_astral_characters_as_two_units,
        sync::an_edit_batch_in_utf8_uses_byte_columns,
        sync::successive_did_changes_increase_the_version_by_one,
        sync::a_full_sync_server_gets_the_full_text_on_every_edit,
        sync::a_server_without_sync_gets_no_did_open_and_the_document_is_unsupported,
        sync::a_replaced_document_sends_one_full_text_did_change,
        sync::closing_a_tab_sends_did_close_for_its_uri,
        sync::rust_analyzer_negotiates_a_position_encoding,
        tokens::did_open_is_followed_by_a_full_request_whose_tokens_reach_the_document,
        tokens::a_range_only_server_gets_a_range_request_for_the_visible_rows,
        tokens::an_edit_on_a_delta_server_sends_one_delta_with_the_last_result_id,
        tokens::an_edit_on_a_server_without_delta_sends_one_full_request,
        tokens::edits_during_a_request_send_exactly_one_follow_up_and_the_stale_answer_is_never_delivered,
        tokens::a_refresh_requests_every_open_document_of_that_server_again,
        tokens::closing_a_document_with_a_request_in_flight_sends_cancel_request_for_it,
        tokens::an_error_answer_delivers_nothing_and_the_next_edit_requests_again,
        tokens::a_delta_beyond_the_cached_tokens_discards_them_and_requests_all_tokens,
        tokens::a_server_without_semantic_tokens_gets_no_request_and_one_debug_line,
        tokens::restart_after_a_crash_starts_the_server_again_and_reopens_its_documents,
        tokens::zls_reports_whether_it_returns_semantic_tokens_and_its_encoding,
        tokens::the_recorded_zls_answer_reaches_the_document_with_every_token_and_its_role,
    ];
    libtest_mimic::run(&Arguments::from_args(), trials).exit();
}
