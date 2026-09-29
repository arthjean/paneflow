use super::*;

fn present<'a>(params: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    params.get(key).filter(|value| !value.is_null())
}

pub(super) fn opt_u64(params: &serde_json::Value, key: &str) -> Result<Option<u64>, JsonRpcError> {
    let Some(value) = present(params, key) else {
        return Ok(None);
    };
    value.as_u64().map(Some).ok_or_else(|| {
        JsonRpcError::invalid_params(format!("'{key}' must be a non-negative integer"))
    })
}

pub(super) fn opt_usize(
    params: &serde_json::Value,
    key: &str,
) -> Result<Option<usize>, JsonRpcError> {
    opt_u64(params, key)?
        .map(|value| {
            usize::try_from(value)
                .map_err(|_| JsonRpcError::invalid_params(format!("'{key}' is out of range")))
        })
        .transpose()
}

pub(super) fn required_index(params: &serde_json::Value) -> Result<usize, JsonRpcError> {
    opt_usize(params, "index")?
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'index' parameter"))
}

pub(super) fn opt_str<'a>(
    params: &'a serde_json::Value,
    key: &str,
) -> Result<Option<&'a str>, JsonRpcError> {
    let Some(value) = present(params, key) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(Some)
        .ok_or_else(|| JsonRpcError::invalid_params(format!("'{key}' must be a string")))
}

pub(super) fn opt_bool(
    params: &serde_json::Value,
    key: &str,
) -> Result<Option<bool>, JsonRpcError> {
    let Some(value) = present(params, key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| JsonRpcError::invalid_params(format!("'{key}' must be a boolean")))
}

pub(super) fn opt_env(
    params: &serde_json::Value,
    key: &str,
) -> Result<Option<HashMap<String, String>>, JsonRpcError> {
    let Some(value) = present(params, key) else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(|| {
        JsonRpcError::invalid_params(format!("'{key}' must be an object of strings"))
    })?;
    let mut env = HashMap::with_capacity(object.len());
    for (name, value) in object {
        let Some(value) = value.as_str() else {
            return Err(JsonRpcError::invalid_params(format!(
                "'{key}.{name}' must be a string"
            )));
        };
        env.insert(name.clone(), value.to_string());
    }
    Ok((!env.is_empty()).then_some(env))
}

pub(super) fn opt_profile(
    params: &serde_json::Value,
) -> Result<TerminalSurfaceProfile, JsonRpcError> {
    match opt_str(params, "profile")? {
        None | Some("normal") => Ok(TerminalSurfaceProfile::Normal),
        Some("agent") => Ok(TerminalSurfaceProfile::Agent),
        Some("cached") => Ok(TerminalSurfaceProfile::Cached),
        Some(other) => Err(JsonRpcError::invalid_params(format!(
            "unknown 'profile' {other:?}; use \"normal\", \"agent\" or \"cached\""
        ))),
    }
}

pub(super) fn opt_label(params: &serde_json::Value) -> Result<Option<String>, JsonRpcError> {
    let label = opt_str(params, "label")?;
    let name = opt_str(params, "name")?;
    Ok(label.or(name).and_then(sanitize_pane_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_reject_strings_negatives_and_decimals() {
        for bad in [
            serde_json::json!({"surface_id": "12"}),
            serde_json::json!({"surface_id": -1}),
            serde_json::json!({"surface_id": 1.5}),
            serde_json::json!({"surface_id": true}),
        ] {
            let error = opt_u64(&bad, "surface_id").expect_err("mistyped id is refused");
            assert_eq!(error.code, JsonRpcError::INVALID_PARAMS, "{bad}");
        }
        assert_eq!(
            opt_u64(&serde_json::json!({"surface_id": 7}), "surface_id"),
            Ok(Some(7))
        );
        assert_eq!(opt_u64(&serde_json::json!({}), "surface_id"), Ok(None));
        assert_eq!(
            opt_u64(&serde_json::json!({"surface_id": null}), "surface_id"),
            Ok(None)
        );
    }

    #[test]
    fn an_index_is_required_and_typed() {
        assert_eq!(required_index(&serde_json::json!({"index": 2})), Ok(2));
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"index": "1"}),
            serde_json::json!({"index": -1}),
        ] {
            assert_eq!(
                required_index(&bad).map_err(|error| error.code),
                Err(JsonRpcError::INVALID_PARAMS),
                "{bad}"
            );
        }
    }

    #[test]
    fn strings_and_booleans_reject_other_types() {
        let params = serde_json::json!({"text": 5, "submit": "yes", "name": ["x"]});
        assert!(opt_str(&params, "text").is_err());
        assert!(opt_bool(&params, "submit").is_err());
        assert!(opt_label(&params).is_err());
        let params = serde_json::json!({"text": "hi", "submit": true});
        assert_eq!(opt_str(&params, "text"), Ok(Some("hi")));
        assert_eq!(opt_bool(&params, "submit"), Ok(Some(true)));
    }

    #[test]
    fn env_must_be_an_object_of_strings() {
        assert!(opt_env(&serde_json::json!({"env": ["A=1"]}), "env").is_err());
        assert!(opt_env(&serde_json::json!({"env": {"PORT": 8080}}), "env").is_err());
        let env = opt_env(&serde_json::json!({"env": {"PORT": "8080"}}), "env")
            .expect("string map")
            .expect("non-empty");
        assert_eq!(env.get("PORT").map(String::as_str), Some("8080"));
        assert_eq!(opt_env(&serde_json::json!({"env": {}}), "env"), Ok(None));
    }

    #[test]
    fn profile_must_be_a_known_value() {
        assert_eq!(
            opt_profile(&serde_json::json!({})),
            Ok(TerminalSurfaceProfile::Normal)
        );
        assert_eq!(
            opt_profile(&serde_json::json!({"profile": "agent"})),
            Ok(TerminalSurfaceProfile::Agent)
        );
        assert!(opt_profile(&serde_json::json!({"profile": "agnet"})).is_err());
        assert!(opt_profile(&serde_json::json!({"profile": 1})).is_err());
    }
}
