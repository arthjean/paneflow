use std::fs::File;
use std::io::{self, Read, Write};

use paneflow_browser_protocol::{read_message, write_message, Controller, Owner};

fn serve(
    controller: &mut Controller,
    scope: &Owner,
    input: &mut impl Read,
    output: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        match read_message(input) {
            Ok(Some(message)) => write_message(output, &controller.dispatch(scope, message))?,
            Ok(None) => return Ok(()),
            Err(error) => return Err(serde_json::to_string(&error)?.into()),
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let deterministic = args.last().is_some_and(|arg| arg == "--deterministic");
    if deterministic {
        args.pop();
    }
    let target = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
    let mut controller = Controller::new(target, deterministic);
    let mut output = io::stdout().lock();
    if args.first().is_some_and(|arg| arg == "--batch")
        && args.len() > 1
        && (args.len() - 1).is_multiple_of(3)
    {
        for channel in args[1..].as_chunks::<3>().0 {
            let scope = Owner {
                workspace: channel[0].clone().try_into()?,
                session: channel[1].clone().try_into()?,
            };
            serve(
                &mut controller,
                &scope,
                &mut File::open(&channel[2])?,
                &mut output,
            )?;
        }
    } else if args.len() == 2 {
        let scope = Owner {
            workspace: args[0].clone().try_into()?,
            session: args[1].clone().try_into()?,
        };
        serve(
            &mut controller,
            &scope,
            &mut io::stdin().lock(),
            &mut output,
        )?;
    } else {
        return Err("usage: paneflow-browser-harness workspace session [--deterministic] | --batch (workspace session framed-file)... [--deterministic]".into());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
