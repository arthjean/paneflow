mod drag;

use cef::*;
use paneflow_browser_protocol::{Document, InputEvent};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;

enum Picker {
    Upload(FileDialogCallback),
    Download(BeforeDownloadCallback),
}
struct Pending {
    document: Document,
    picker: Picker,
}
struct Download {
    document: Document,
    request: u64,
    callback: Option<DownloadItemCallback>,
    canceled: bool,
}
thread_local! {
    static PENDING: RefCell<BTreeMap<u64, Pending>> = const { RefCell::new(BTreeMap::new()) };
    static DOWNLOADS: RefCell<BTreeMap<u32, Download>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT: RefCell<u64> = const { RefCell::new(1) };
}
fn insert(document: Document, picker: Picker) -> Option<u64> {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending.len() >= 8 {
            if let Picker::Upload(callback) = picker {
                callback.cancel();
            }
            return None;
        }
        let request = NEXT.with(|next| {
            let mut next = next.borrow_mut();
            let id = *next;
            *next = next.saturating_add(1);
            id
        });
        pending.insert(request, Pending { document, picker });
        Some(request)
    })
}
fn cancel_matching(matches: impl Fn(&Download) -> bool) {
    let callbacks = DOWNLOADS.with(|downloads| {
        downloads
            .borrow_mut()
            .values_mut()
            .filter(|item| matches(item))
            .filter_map(|item| {
                item.canceled = true;
                item.callback.clone()
            })
            .collect::<Vec<_>>()
    });
    for callback in callbacks {
        callback.cancel();
    }
}
pub fn clear(document: &Document) {
    drag::cancel(document);
    let removed = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let ids: Vec<_> = pending
            .iter()
            .filter(|(_, item)| item.document.browser == document.browser)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| pending.remove(&id))
            .collect::<Vec<_>>()
    });
    for item in removed {
        if let Picker::Upload(callback) = item.picker {
            callback.cancel();
        }
    }
    cancel_matching(|item| item.document.browser == document.browser);
}
pub fn handle(document: &Document, input: &InputEvent) {
    if !input.is_valid() {
        return;
    }
    if let InputEvent::CancelDownload { request } = input {
        cancel_matching(|item| item.document == *document && item.request == *request);
        return;
    }
    let InputEvent::TransferResponse { request, paths } = input else {
        return;
    };
    let pending = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending
            .get(request)
            .is_some_and(|item| item.document == *document)
        {
            pending.remove(request)
        } else {
            None
        }
    });
    let Some(pending) = pending else { return };
    match pending.picker {
        Picker::Upload(callback) => {
            if paths.is_empty() {
                callback.cancel();
            } else {
                let mut files = CefStringList::new();
                if paths.iter().all(|path| files.append(path)) {
                    callback.cont(Some(&mut files));
                } else {
                    callback.cancel();
                }
            }
        }
        Picker::Download(callback) => {
            if paths.len() == 1 {
                callback.cont(Some(&paths[0].as_str().into()), 0);
            } else {
                handle(document, &InputEvent::CancelDownload { request: *request });
            }
        }
    }
}
wrap_dialog_handler! {
    pub struct Uploads { document: Document }
    impl DialogHandler {
        fn on_file_dialog(&self, _browser: Option<&mut Browser>, mode: FileDialogMode, _title: Option<&CefString>, _default_file_path: Option<&CefString>, _accept_filters: Option<&mut CefStringList>, _accept_extensions: Option<&mut CefStringList>, _accept_descriptions: Option<&mut CefStringList>, callback: Option<&mut FileDialogCallback>) -> i32 {
            let Some(callback) = callback else { return 1 };
            let Some(document) = super::latest_document(&self.document) else { callback.cancel(); return 1 };
            if let Some(request) = insert(document.clone(), Picker::Upload(callback.clone())) {
                super::emit(json!({"native":"file_picker","document":document,"request":request,"multiple":mode == FileDialogMode::OPEN_MULTIPLE,"directory":mode == FileDialogMode::OPEN_FOLDER}));
            }
            1
        }
    }
}
wrap_download_handler! {
    pub struct Downloads { document: Document }
    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> i32 { 1 }
        fn on_before_download(&self, _browser: Option<&mut Browser>, download_item: Option<&mut DownloadItem>, suggested_name: Option<&CefString>, callback: Option<&mut BeforeDownloadCallback>) -> i32 {
            let (Some(item), Some(callback), Some(document)) = (download_item, callback, super::latest_document(&self.document)) else { return 1 };
            if DOWNLOADS.with(|downloads| downloads.borrow().len() >= 32) { return 1; }
            if let Some(request) = insert(document.clone(), Picker::Download(callback.clone())) {
                DOWNLOADS.with(|downloads| { downloads.borrow_mut().insert(item.id(), Download { document: document.clone(), request, callback: None, canceled: false }); });
                let name = suggested_name.map(ToString::to_string).unwrap_or_default();
                let name = std::path::Path::new(&name).file_name().and_then(|name| name.to_str()).unwrap_or("download").chars().take(255).collect::<String>();
                super::emit(json!({"native":"download_destination","document":document,"request":request,"suggested_name":name}));
            }
            1
        }
        fn on_download_updated(&self, _browser: Option<&mut Browser>, download_item: Option<&mut DownloadItem>, callback: Option<&mut DownloadItemCallback>) {
            let Some(item) = download_item else { return };
            let cancel = DOWNLOADS.with(|downloads| {
                let mut downloads = downloads.borrow_mut();
                let download = downloads.get_mut(&item.id())?;
                download.callback = callback.cloned();
                let cancel = if download.canceled && item.is_canceled() == 0 { download.callback.clone() } else { None };
                super::emit(json!({"native":"download_progress","document":download.document,"request":download.request,"received":item.received_bytes(),"total":item.total_bytes(),"complete":item.is_complete() != 0,"canceled":item.is_canceled() != 0}));
                if item.is_complete() != 0 || item.is_canceled() != 0 { downloads.remove(&item.id()); }
                cancel
            });
            if let Some(callback) = cancel { callback.cancel(); }
        }
    }
}

pub fn drop_files(host: &BrowserHost, paths: &[String], x: i32, y: i32) {
    drag::begin(host, paths, x, y);
}

pub fn update_drag_cursor(browser: &Browser, operation: DragOperationsMask) {
    drag::cursor(browser, operation);
}

pub fn closed_drag(browser: &Browser) {
    drag::closed(browser);
}

#[cfg(test)]
mod tests {
    use paneflow_browser_protocol::InputEvent;

    #[test]
    fn os_file_drop_rejects_unbounded_paths_and_coordinates() {
        assert!(InputEvent::DropFiles {
            paths: vec!["/chosen.txt".into()],
            x: 0,
            y: 32768
        }
        .is_valid());
        for (paths, x, y) in [
            (Vec::new(), 0, 0),
            (vec!["a".repeat(4097)], 0, 0),
            (vec!["a".into()], -1, 0),
            (vec!["a".into()], 0, 32769),
        ] {
            assert!(!InputEvent::DropFiles { paths, x, y }.is_valid());
        }
    }

    #[test]
    fn transfer_response_bounds_paths_and_allows_human_cancellation() {
        assert!(InputEvent::TransferResponse {
            request: 1,
            paths: Vec::new()
        }
        .is_valid());
        for paths in [
            vec!["bad\0path".to_owned()],
            vec!["x".repeat(4097)],
            vec!["x".to_owned(); 65],
            vec!["x".repeat(4096); 33],
        ] {
            assert!(!InputEvent::TransferResponse { request: 1, paths }.is_valid());
        }
        assert!(!InputEvent::TransferResponse {
            request: 0,
            paths: Vec::new()
        }
        .is_valid());
        assert!(InputEvent::TransferResponse {
            request: 1,
            paths: vec!["/home/user/chosen.txt".to_owned()]
        }
        .is_valid());
        assert!(!InputEvent::CancelDownload { request: 0 }.is_valid());
    }
}
