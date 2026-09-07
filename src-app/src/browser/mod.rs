mod address;
pub mod authority;
mod benchmark;
mod ime;
pub mod input;
#[cfg(test)]
mod input_tests;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod page;
#[cfg(target_os = "linux")]
pub mod presentation;
pub mod profile;
#[cfg(target_os = "linux")]
pub mod prototype;
#[cfg(target_os = "linux")]
pub mod supervisor;
pub mod view;

pub const PROTOTYPE_VERB: &str = "browser-prototype";
pub const HOST_ENV: &str = "PANEFLOW_BROWSER_HOST";
pub const RUNTIME_ENV: &str = "PANEFLOW_CEF_ROOT";

#[cfg(target_os = "linux")]
pub use prototype::run as run_prototype;

#[cfg(not(target_os = "linux"))]
pub fn run_prototype(_args: &[String]) -> i32 {
    eprintln!(
        "browser unavailable: the GPU presentation prototype has a Linux adapter only; external URL opening and terminals are unaffected"
    );
    2
}

#[cfg(target_os = "linux")]
pub struct BrowserRuntime {
    supervisor: supervisor::HostSupervisor,
    events: std::cell::RefCell<Option<smol::channel::Receiver<supervisor::HostEvent>>>,
    pages: std::sync::Mutex<Vec<supervisor::HostSupervisor>>,
}

#[cfg(target_os = "linux")]
impl gpui::Global for BrowserRuntime {}

#[cfg(target_os = "linux")]
impl BrowserRuntime {
    pub fn install(cx: &mut gpui::App) {
        let (supervisor, receiver) = supervisor::HostSupervisor::channel();
        cx.set_global(Self {
            supervisor,
            events: std::cell::RefCell::new(Some(receiver)),
            pages: std::sync::Mutex::new(Vec::new()),
        });
        cx.on_app_quit(|cx| {
            let runtime = cx.global::<Self>();
            let mut supervisors = vec![runtime.supervisor.clone()];
            supervisors.extend(
                runtime
                    .pages
                    .lock()
                    .map(|pages| pages.clone())
                    .unwrap_or_default(),
            );
            async move {
                smol::unblock(move || {
                    for supervisor in supervisors {
                        supervisor.shutdown(supervisor::SHUTDOWN_GRACE);
                    }
                })
                .await;
            }
        })
        .detach();
    }

    pub fn register_page(&self, supervisor: supervisor::HostSupervisor) {
        if let Ok(mut pages) = self.pages.lock() {
            pages.retain(|page| page.state() != supervisor::HostState::Inactive);
            pages.push(supervisor);
        }
    }

    pub fn supervisor(&self) -> &supervisor::HostSupervisor {
        &self.supervisor
    }

    pub fn take_events(&self) -> Option<smol::channel::Receiver<supervisor::HostEvent>> {
        self.events.borrow_mut().take()
    }
}

pub(crate) fn origin_of(url: &str) -> Result<String, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("the URL must be an absolute http or https URL")?;
    if scheme != "http" && scheme != "https" {
        return Err("the URL must use http or https".to_string());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("the URL needs a plain host".to_string());
    }
    Ok(format!("{scheme}://{authority}"))
}
