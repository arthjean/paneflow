#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fake_server;
mod lifecycle;
mod support;
mod sync;

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
    ];
    libtest_mimic::run(&Arguments::from_args(), trials).exit();
}
