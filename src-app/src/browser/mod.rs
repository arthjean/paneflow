mod accessibility;
mod address;
pub(crate) mod agent;
pub mod authority;
mod benchmark;
#[cfg(target_os = "windows")]
#[path = "display_scale_windows.rs"]
pub mod display_scale;
mod ime;
pub mod input;
#[cfg(test)]
mod input_tests;
#[cfg(target_os = "linux")]
pub mod install;
#[cfg(target_os = "windows")]
#[path = "install_windows.rs"]
pub mod install;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod page;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod presentation;
pub mod profile;
#[cfg(target_os = "linux")]
mod profile_host;
#[cfg(target_os = "windows")]
#[path = "profile_host_windows.rs"]
mod profile_host;
#[cfg(target_os = "linux")]
pub mod prototype;
#[cfg(target_os = "windows")]
#[path = "prototype_windows.rs"]
pub mod prototype;
#[cfg(target_os = "linux")]
pub mod security_corpus;
#[cfg(target_os = "linux")]
pub mod supervisor;
#[cfg(target_os = "windows")]
#[path = "supervisor_windows.rs"]
pub mod supervisor;
pub mod view;
#[cfg(target_os = "windows")]
pub mod windows;

pub const PROTOTYPE_VERB: &str = "browser-prototype";
pub const SECURITY_CORPUS_VERB: &str = "browser-security-corpus";
pub const HOST_ENV: &str = "PANEFLOW_BROWSER_HOST";
pub const RUNTIME_ENV: &str = "PANEFLOW_CEF_ROOT";

#[cfg(target_os = "linux")]
pub use prototype::run as run_prototype;
#[cfg(target_os = "windows")]
pub use prototype::run as run_prototype;
#[cfg(target_os = "linux")]
pub use security_corpus::run as run_security_corpus;

#[cfg(not(target_os = "linux"))]
pub fn run_security_corpus() -> i32 {
    eprintln!("browser unavailable: the security corpus describes the Linux browser only");
    2
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
pub fn run_prototype(_args: &[String]) -> i32 {
    eprintln!(
        "browser unavailable: the GPU presentation prototype has a Linux adapter only; external URL opening and terminals are unaffected"
    );
    2
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub struct BrowserRuntime {
    supervisor: supervisor::HostSupervisor,
    #[cfg(target_os = "linux")]
    events: std::cell::RefCell<Option<smol::channel::Receiver<supervisor::HostEvent>>>,
    pages: std::sync::Mutex<Vec<supervisor::HostSupervisor>>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl gpui::Global for BrowserRuntime {}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl BrowserRuntime {
    pub fn install(cx: &mut gpui::App) {
        let (supervisor, receiver) = supervisor::HostSupervisor::channel();
        cx.set_global(Self {
            supervisor,
            #[cfg(target_os = "linux")]
            events: std::cell::RefCell::new(Some(receiver)),
            pages: std::sync::Mutex::new(Vec::new()),
        });
        #[cfg(not(target_os = "linux"))]
        drop(receiver);
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

    #[cfg(target_os = "linux")]
    pub fn supervisor(&self) -> &supervisor::HostSupervisor {
        &self.supervisor
    }

    #[cfg(target_os = "linux")]
    pub fn live_hosts(&self) -> usize {
        let live = |supervisor: &supervisor::HostSupervisor| {
            supervisor.state() != supervisor::HostState::Inactive
        };
        usize::from(live(&self.supervisor))
            + self
                .pages
                .lock()
                .map(|pages| pages.iter().filter(|page| live(page)).count())
                .unwrap_or(0)
    }

    #[cfg(target_os = "linux")]
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
