use std::ffi::OsString;
use std::path::Path;

use serde_json::{json, Value};

use crate::transport::{send_request, HOOK_REQUEST_DEADLINE};

pub(crate) const RESUME_ATTEMPT_ARG: &str = "resume-attempt";
pub(crate) const METHOD_RESUME_ATTEMPT: &str = "session.resume_attempt";

pub(crate) fn params(session: &str, fields: &[OsString]) -> Option<Value> {
    let fields: Vec<&str> = fields
        .iter()
        .map(|field| field.to_str())
        .collect::<Option<_>>()?;
    match fields.as_slice() {
        [attempt, "started", pid] => Some(json!({
            "session": session,
            "attempt": attempt,
            "outcome": "started",
            "pid": pid.parse::<u32>().ok()?,
        })),
        [attempt, "failed", reason] => Some(json!({
            "session": session,
            "attempt": attempt,
            "outcome": "failed",
            "reason": reason,
        })),
        _ => None,
    }
}

pub(crate) fn relay(endpoint: &Path, session: &str, fields: &[OsString]) -> Result<(), String> {
    let params = params(session, fields).ok_or("malformed resume attempt arguments")?;
    send_request(
        endpoint,
        METHOD_RESUME_ATTEMPT,
        params,
        HOOK_REQUEST_DEADLINE,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_launchers_two_report_shapes_become_resume_attempt_params() {
        let attempt = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            params("s-1", &fields(&[attempt, "started", "4242"])),
            Some(json!({"session": "s-1", "attempt": attempt, "outcome": "started", "pid": 4242}))
        );
        assert_eq!(
            params("s-1", &fields(&[attempt, "failed", "not_found"])),
            Some(
                json!({"session": "s-1", "attempt": attempt, "outcome": "failed", "reason": "not_found"})
            )
        );
        assert_eq!(params("s-1", &fields(&[attempt, "started", "x"])), None);
        assert_eq!(params("s-1", &fields(&[attempt, "exploded", "x"])), None);
        assert_eq!(params("s-1", &fields(&[attempt])), None);
    }
}
