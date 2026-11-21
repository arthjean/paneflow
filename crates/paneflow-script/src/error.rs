use std::fmt;
use std::sync::Arc;

use mlua::Lua;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Halt {
    Stop(i32),
    Fatal(String),
    Interrupted,
    Expired,
}

impl fmt::Display for Halt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop(code) => write!(formatter, "the script stopped with status {code}"),
            Self::Fatal(message) => formatter.write_str(message),
            Self::Interrupted => formatter.write_str("interrupted"),
            Self::Expired => formatter.write_str("the time given with --for ran out"),
        }
    }
}

impl std::error::Error for Halt {}

pub fn halt_of(error: &mlua::Error) -> Option<Halt> {
    match error {
        mlua::Error::ExternalError(inner) => inner.downcast_ref::<Halt>().cloned(),
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => {
            halt_of(cause)
        }
        mlua::Error::BadArgument { cause, .. } => halt_of(cause),
        _ => None,
    }
}

pub fn halt(halt: Halt) -> mlua::Error {
    mlua::Error::ExternalError(Arc::new(halt))
}

fn without_traceback(message: &str) -> &str {
    message
        .split_once("\nstack traceback:")
        .map_or(message, |(head, _)| head)
}

pub fn describe(error: &mlua::Error) -> String {
    match error {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(message) => without_traceback(message).to_string(),
        mlua::Error::MemoryError(_) => {
            "not enough memory: the script exceeded its memory limit".to_string()
        }
        mlua::Error::CallbackError { cause, .. } => describe(cause),
        mlua::Error::WithContext { context, cause } => format!("{context}: {}", describe(cause)),
        mlua::Error::BadArgument { to, pos, cause, .. } => match to {
            Some(function) => format!("bad argument #{pos} to {function}: {}", describe(cause)),
            None => format!("bad argument #{pos}: {}", describe(cause)),
        },
        mlua::Error::ExternalError(inner) => inner.to_string(),
        other => other.to_string(),
    }
}

pub const INTERNAL_CHUNK: &str = "[paneflow]";

pub const MLUA_CHUNK: &str = "__mlua";

pub fn located(lua: &Lua, message: impl fmt::Display) -> mlua::Error {
    mlua::Error::RuntimeError(format!("{}{message}", location(lua)))
}

pub fn location(lua: &Lua) -> String {
    for level in 1..8 {
        let found = lua.inspect_stack(level, |debug| {
            let source = debug.source();
            let line = debug.current_line();
            match (source.what, source.short_src.as_deref(), line) {
                ("Lua" | "main", Some(source), Some(line))
                    if !source.starts_with(INTERNAL_CHUNK) && !source.starts_with(MLUA_CHUNK) =>
                {
                    Some(format!("{source}:{line}: "))
                }
                _ => None,
            }
        });
        match found {
            Some(Some(location)) => return location,
            Some(None) => continue,
            None => break,
        }
    }
    String::new()
}
