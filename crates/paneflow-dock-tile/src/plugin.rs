use std::cell::RefCell;
use std::ffi::{CStr, c_void};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{NSDockTile, NSDockTilePlugIn, NSImage, NSImageScaling, NSImageView};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSDistributedNotificationCenter, NSNotification, NSPoint, NSRect,
    NSString, NSUserDefaults, ns_string,
};

use crate::contract;

pub const CLASS_NAME: &CStr = c"PaneflowDockTilePlugIn";

pub struct Ivars {
    dock_tile: RefCell<Option<Retained<NSDockTile>>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "PaneflowDockTilePlugIn"]
    #[ivars = Ivars]
    pub struct DockTilePlugIn;

    impl DockTilePlugIn {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars {
                dock_tile: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(appIconChanged:))]
        fn app_icon_changed(&self, _notification: &NSNotification) {
            guard(|| self.schedule_refresh());
        }

        #[unsafe(method(refreshDockTile))]
        fn refresh_dock_tile(&self) {
            guard(|| self.refresh());
        }
    }

    unsafe impl NSObjectProtocol for DockTilePlugIn {}

    unsafe impl NSDockTilePlugIn for DockTilePlugIn {
        #[unsafe(method(setDockTile:))]
        fn set_dock_tile(&self, dock_tile: Option<&NSDockTile>) {
            guard(|| self.attach(dock_tile));
        }
    }
);

fn guard(work: impl FnOnce()) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(work));
}

impl DockTilePlugIn {
    fn observer(&self) -> &AnyObject {
        self.as_ref()
    }

    fn attach(&self, dock_tile: Option<&NSDockTile>) {
        let center = NSDistributedNotificationCenter::defaultCenter();
        unsafe { center.removeObserver(self.observer()) };
        let Some(dock_tile) = dock_tile else {
            self.ivars().dock_tile.replace(None);
            return;
        };
        self.ivars().dock_tile.replace(Some(dock_tile.retain()));
        if let Some(host) = host_identifier() {
            let name = NSString::from_str(&contract::notification_name(&host));
            unsafe {
                center.addObserver_selector_name_object(
                    self.observer(),
                    sel!(appIconChanged:),
                    Some(&name),
                    None,
                );
            }
        }
        self.schedule_refresh();
    }

    fn schedule_refresh(&self) {
        if MainThreadMarker::new().is_some() {
            self.refresh();
            return;
        }
        let nothing: Option<&AnyObject> = None;
        let _: () = unsafe {
            msg_send![
                self,
                performSelectorOnMainThread: sel!(refreshDockTile),
                withObject: nothing,
                waitUntilDone: false
            ]
        };
    }

    fn refresh(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(dock_tile) = self.ivars().dock_tile.borrow().clone() else {
            return;
        };
        match chosen_image() {
            Some(image) => {
                let frame = NSRect::new(NSPoint::new(0.0, 0.0), dock_tile.size());
                let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
                view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                view.setImage(Some(&image));
                dock_tile.setContentView(Some(&view));
            }
            None => dock_tile.setContentView(None),
        }
        dock_tile.display();
    }
}

pub fn image_path(address: usize) -> Option<PathBuf> {
    let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
    if unsafe { libc::dladdr(address as *const c_void, info.as_mut_ptr()) } == 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(info.dli_fname) };
    Some(PathBuf::from(name.to_str().ok()?))
}

fn plugin_path() -> Option<PathBuf> {
    let executable = image_path(plugin_path as fn() -> Option<PathBuf> as usize)?;
    contract::bundle_of_executable(&executable).map(Path::to_path_buf)
}

fn plugin_bundle() -> Option<Retained<NSBundle>> {
    NSBundle::bundleWithPath(&NSString::from_str(plugin_path()?.to_str()?))
}

fn host_identifier() -> Option<String> {
    let plugin = plugin_path()?;
    let host = contract::host_app(&plugin)?;
    let app = NSBundle::bundleWithPath(&NSString::from_str(host.to_str()?))?;
    Some(app.bundleIdentifier()?.to_string())
}

fn chosen_image() -> Option<Retained<NSImage>> {
    let host = host_identifier()?;
    let defaults = NSUserDefaults::initWithSuiteName(
        NSUserDefaults::alloc(),
        Some(&NSString::from_str(&host)),
    )?;
    let name = defaults
        .stringForKey(&NSString::from_str(contract::APP_ICON_KEY))?
        .to_string();
    if !contract::is_icon_name(&name) {
        return None;
    }
    let path = plugin_bundle()?
        .pathForResource_ofType(Some(&NSString::from_str(&name)), Some(ns_string!("png")))?;
    NSImage::initWithContentsOfFile(NSImage::alloc(), &path)
}

pub fn publish(icon: Option<&str>) {
    let Some(identifier) = NSBundle::mainBundle().bundleIdentifier() else {
        return;
    };
    publish_to(
        &NSUserDefaults::standardUserDefaults(),
        &identifier.to_string(),
        icon,
    );
}

pub fn publish_to(defaults: &NSUserDefaults, host_identifier: &str, icon: Option<&str>) {
    let key = NSString::from_str(contract::APP_ICON_KEY);
    match icon {
        Some(name) => {
            let value = NSString::from_str(name);
            unsafe { defaults.setObject_forKey(Some(value.as_ref()), &key) };
        }
        None => defaults.removeObjectForKey(&key),
    }
    defaults.synchronize();
    let name = NSString::from_str(&contract::notification_name(host_identifier));
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true);
    }
}

#[cfg(feature = "bundle")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static REGISTER_ON_LOAD: extern "C" fn() = register_on_load;

#[cfg(feature = "bundle")]
extern "C" fn register_on_load() {
    guard(|| {
        register();
    });
}

pub fn register() -> &'static AnyClass {
    AnyClass::get(CLASS_NAME).unwrap_or_else(DockTilePlugIn::class)
}
