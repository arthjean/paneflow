#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    match probe::run() {
        Ok(()) => {
            println!("dock tile plug-in: loaded, shows the chosen icon and follows its changes");
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("dock tile plug-in: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
mod probe {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, sel};
    use objc2_app_kit::{NSApplication, NSDockTile, NSImageView};
    use objc2_foundation::{
        MainThreadMarker, NSBundle, NSDate, NSRunLoop, NSString, NSUserDefaults,
    };
    use paneflow_dock_tile::{CLASS_NAME, contract, image_path, publish_to};

    const SAMPLE_ICON: &str = "chip";
    const NOTIFICATION_DEADLINE: Duration = Duration::from_secs(10);
    const RUN_LOOP_SLICE_SECONDS: f64 = 0.05;

    pub fn run() -> Result<(), String> {
        let argument = std::env::args()
            .nth(1)
            .ok_or("usage: probe <path to PaneflowDockTile.docktileplugin>")?;
        let path = std::fs::canonicalize(&argument)
            .map_err(|error| format!("the plug-in path {argument} does not resolve: {error}"))?
            .into_os_string()
            .into_string()
            .map_err(|_| "the plug-in path is not UTF-8")?;
        let mtm = MainThreadMarker::new().ok_or("the probe must run on the main thread")?;
        let host_path = contract::host_app(Path::new(&path))
            .ok_or("the plug-in must sit in an app's Contents/PlugIns")?;
        let host = NSBundle::bundleWithPath(&NSString::from_str(
            host_path.to_str().ok_or("the host path is not UTF-8")?,
        ))
        .ok_or("the host app bundle does not open")?;
        let host_id = host
            .bundleIdentifier()
            .ok_or("the host app has no bundle identifier")?
            .to_string();

        let bundle = NSBundle::bundleWithPath(&NSString::from_str(&path))
            .ok_or("the plug-in bundle does not open")?;
        if !unsafe { bundle.load() } {
            return Err("the plug-in executable does not load".into());
        }
        let class = bundle.principalClass().ok_or(
            "the plug-in names no principal class, or the class was not registered on load",
        )?;
        if class.name() != CLASS_NAME {
            return Err(format!("the principal class is {:?}", class.name()));
        }
        let set_dock_tile = class
            .instance_method(sel!(setDockTile:))
            .ok_or("the principal class does not implement setDockTile:")?;
        let code = set_dock_tile.implementation() as usize;
        let owner = image_path(code).ok_or("the code of setDockTile: maps to no image")?;
        let owner = owner.canonicalize().unwrap_or(owner);
        if !owner.starts_with(&path) {
            return Err(format!(
                "setDockTile: runs code from {}, not from the plug-in",
                owner.display()
            ));
        }

        let defaults = NSUserDefaults::initWithSuiteName(
            NSUserDefaults::alloc(),
            Some(&NSString::from_str(&host_id)),
        )
        .ok_or("the host's preferences do not open")?;
        publish_to(&defaults, &host_id, Some(SAMPLE_ICON));

        let plugin: Retained<AnyObject> = unsafe { msg_send![class, new] };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        let _: () = unsafe { msg_send![&*plugin, setDockTile: &*tile] };
        wait_for(mtm, &tile, true, "the chosen icon on attach")?;

        publish_to(&defaults, &host_id, None);
        wait_for(mtm, &tile, false, "the installed icon after Glass")?;

        publish_to(&defaults, &host_id, Some(SAMPLE_ICON));
        wait_for(mtm, &tile, true, "the chosen icon after a change")?;

        let detached: Option<&NSDockTile> = None;
        let _: () = unsafe { msg_send![&*plugin, setDockTile: detached] };
        defaults.removeObjectForKey(&NSString::from_str(contract::APP_ICON_KEY));
        Ok(())
    }

    fn shows_icon(mtm: MainThreadMarker, tile: &NSDockTile) -> bool {
        tile.contentView(mtm)
            .and_then(|view| view.downcast::<NSImageView>().ok())
            .and_then(|view| view.image())
            .is_some()
    }

    fn wait_for(
        mtm: MainThreadMarker,
        tile: &NSDockTile,
        icon: bool,
        what: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + NOTIFICATION_DEADLINE;
        while shows_icon(mtm, tile) != icon {
            if Instant::now() > deadline {
                return Err(format!("the Dock tile never showed {what}"));
            }
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(
                RUN_LOOP_SLICE_SECONDS,
            ));
        }
        Ok(())
    }
}
