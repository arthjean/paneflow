use std::io::{self, BufReader};
use std::sync::{Arc, Mutex};

use cef::*;
use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};
use interprocess::TryClone;
use paneflow_browser_protocol::{
    read_message, write_message, Command, Controller, Envelope, Owner, Reply, CONTRACT_VERSION,
};
use serde_json::{json, Value};

wrap_app! {
    struct BrowserApp;

    impl App {}
}

fn emit(stream: &Arc<Mutex<Stream>>, value: &Value) -> io::Result<()> {
    let mut output = stream
        .lock()
        .map_err(|_| io::Error::other("browser output lock poisoned"))?;
    write_message(&mut *output, value)
}

fn owner() -> Result<Owner, String> {
    let value = std::env::var("PANEFLOW_BROWSER_OWNER")
        .map_err(|_| "PANEFLOW_BROWSER_OWNER is required".to_string())?;
    let (workspace, session) = value
        .split_once('/')
        .ok_or("PANEFLOW_BROWSER_OWNER must be workspace/session")?;
    Ok(Owner {
        workspace: workspace
            .to_owned()
            .try_into()
            .map_err(|_| "invalid browser workspace identity")?,
        session: session
            .to_owned()
            .try_into()
            .map_err(|_| "invalid browser session identity")?,
    })
}

fn dispatch(controller: &mut Controller, caller: &Owner, message: Envelope) -> Reply {
    controller.dispatch(caller, message)
}

fn run_control(stream: Stream, owner: Owner) {
    let output = Arc::new(Mutex::new(stream.try_clone().ok().unwrap_or(stream)));
    let mut input = BufReader::new(
        match output
            .lock()
            .ok()
            .and_then(|stream| stream.try_clone().ok())
        {
            Some(stream) => stream,
            None => return,
        },
    );
    let mut controller = Controller::new(format!("{}-windows", std::env::consts::ARCH), true);
    loop {
        let message = match read_message(&mut input) {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => {
                quit_message_loop();
                break;
            }
        };
        if matches!(message.command, Command::Close { .. }) {
            let reply = dispatch(&mut controller, &owner, message);
            let _ = emit(&output, &json!({ "protocol": reply }));
            quit_message_loop();
            break;
        }
        let reply = dispatch(&mut controller, &owner, message);
        let _ = emit(&output, &json!({ "protocol": reply }));
    }
}

fn connect_control() -> Result<Stream, String> {
    let path = std::env::var_os("PANEFLOW_BROWSER_CONTROL_PIPE")
        .ok_or("PANEFLOW_BROWSER_CONTROL_PIPE is required")?;
    let path = std::path::PathBuf::from(path);
    let name = path
        .to_fs_name::<GenericFilePath>()
        .map_err(|error| format!("browser named-pipe name: {error}"))?;
    Stream::connect(name).map_err(|error| format!("browser named-pipe connect: {error}"))
}

fn run(instance: sys::HINSTANCE, sandbox_info: *mut u8) -> i32 {
    let main_args = MainArgs { instance };
    let args = cef::args::Args::from(main_args);
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let Some(_) = args.as_cmd_line() else {
        return 1;
    };
    let exit_code = execute_process(Some(args.as_main_args()), None, sandbox_info);
    if exit_code >= 0 {
        return exit_code;
    }
    let profile = std::env::var_os("PANEFLOW_CEF_PROFILE").map(std::path::PathBuf::from);
    let runtime = std::env::var_os("PANEFLOW_CEF_ROOT").map(std::path::PathBuf::from);
    let release = runtime.map(|path| path.join("Release"));
    let settings = Settings {
        no_sandbox: 0,
        command_line_args_disabled: 1,
        windowless_rendering_enabled: 0,
        root_cache_path: profile
            .as_ref()
            .map(|path| path.to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        cache_path: profile
            .as_ref()
            .map(|path| path.join("profile").to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        resources_dir_path: release
            .as_ref()
            .map(|path| path.to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        locales_dir_path: release
            .as_ref()
            .map(|path| path.join("locales").to_string_lossy().as_ref().into())
            .unwrap_or_default(),
        ..Default::default()
    };
    let mut application = BrowserApp::new();
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut application),
        sandbox_info,
    ) != 1
    {
        return 1;
    }
    let stream = match connect_control() {
        Ok(stream) => stream,
        Err(_) => {
            shutdown();
            return 1;
        }
    };
    let owner = match owner() {
        Ok(owner) => owner,
        Err(_) => {
            shutdown();
            return 1;
        }
    };
    let handshake = Envelope {
        version: CONTRACT_VERSION,
        operation: "hello"
            .to_owned()
            .try_into()
            .unwrap_or_else(|_| unreachable!()),
        command: Command::Capabilities,
    };
    let mut controller = Controller::new(format!("{}-windows", std::env::consts::ARCH), true);
    let hello = dispatch(&mut controller, &owner, handshake);
    let initialized = json!({
        "native": "initialized",
        "pid": std::process::id(),
        "sandbox_requested": true,
        "contract_version": CONTRACT_VERSION,
        "presentation": false,
        "protocol": hello,
    });
    let output = Arc::new(Mutex::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => {
            shutdown();
            return 1;
        }
    }));
    if emit(&output, &initialized).is_err() {
        shutdown();
        return 1;
    }
    std::thread::spawn(move || run_control(stream, owner));
    run_message_loop();
    shutdown();
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn RunWinMain(
    instance: sys::HINSTANCE,
    _command_line: *const u8,
    _command_show: i32,
    sandbox_info: *mut u8,
) -> i32 {
    run(instance, sandbox_info)
}
