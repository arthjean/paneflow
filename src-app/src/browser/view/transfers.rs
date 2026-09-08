use super::*;

impl BrowserView {
    pub(super) fn drop_external_files(
        &mut self,
        paths: &gpui::ExternalPaths,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        super::super::benchmark::record(
            self.id.as_str(),
            "drop_received",
            serde_json::json!({"count":paths.paths().len(), "live": self.live.is_some(), "inside":self.viewport.is_some_and(|bounds| bounds.contains(&window.mouse_position()))}),
        );
        let (Some(live), Some(bounds)) = (&self.live, self.viewport) else {
            return;
        };
        let position = window.mouse_position();
        if !bounds.contains(&position) {
            return;
        }
        let (x, y) = relative_position(position, bounds.origin);
        let Ok(paths) = paths
            .paths()
            .iter()
            .map(|path| path.to_str().map(str::to_owned).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
        else {
            return;
        };
        let input = InputEvent::DropFiles { paths, x, y };
        if input.is_valid() {
            let result = live.send_to_document(|document| Command::Input { document, input });
            super::super::benchmark::record(
                self.id.as_str(),
                "drop_sent",
                serde_json::json!({"x":x,"y":y,"accepted":result.is_ok()}),
            );
        }
    }

    pub(super) fn handle_transfer(
        &mut self,
        value: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(kind) = value.get("native").and_then(serde_json::Value::as_str) else {
            return false;
        };
        if !matches!(
            kind,
            "file_picker" | "download_destination" | "download_progress"
        ) {
            return false;
        }
        let Some(document) = value
            .get("document")
            .and_then(|value| {
                serde_json::from_value::<paneflow_browser_protocol::Document>(value.clone()).ok()
            })
            .filter(|document| self.live.as_ref().and_then(LivePage::document) == Some(document))
        else {
            return true;
        };
        let Some(request) = value
            .get("request")
            .and_then(serde_json::Value::as_u64)
            .filter(|request| *request > 0)
        else {
            return true;
        };
        if kind == "download_progress" {
            let complete = value
                .get("complete")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let canceled = value
                .get("canceled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if complete || canceled {
                self.download_progress.remove(&request);
            } else if self.download_progress.contains_key(&request)
                || self.download_progress.len() < 32
            {
                self.download_progress.insert(
                    request,
                    (
                        value
                            .get("received")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        value
                            .get("total")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(-1),
                    ),
                );
            }
            cx.notify();
            return true;
        }
        if kind == "file_picker" {
            let directory = value
                .get("directory")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: !directory,
                directories: directory,
                multiple: value
                    .get("multiple")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                prompt: None,
            });
            cx.spawn(async move |this, cx| {
                let paths = receiver
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .unwrap_or_default();
                let _ = this.update(cx, |view, _cx| {
                    view.respond_transfer(document, request, paths)
                });
            })
            .detach();
        } else {
            let directory = dirs::download_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_default();
            let name = value
                .get("suggested_name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| name.len() <= 1024 && !name.contains(['/', '\\', '\0']))
                .unwrap_or("download");
            let receiver = cx.prompt_for_new_path(&directory, Some(name));
            cx.spawn(async move |this, cx| {
                let paths = receiver
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .into_iter()
                    .collect();
                let _ = this.update(cx, |view, _cx| {
                    view.respond_transfer(document, request, paths)
                });
            })
            .detach();
        }
        true
    }

    fn respond_transfer(
        &self,
        document: paneflow_browser_protocol::Document,
        request: u64,
        paths: Vec<std::path::PathBuf>,
    ) {
        let Some(live) = self
            .live
            .as_ref()
            .filter(|live| live.document() == Some(&document))
        else {
            return;
        };
        let paths = paths
            .into_iter()
            .map(|path| path.into_os_string().into_string())
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default();
        let input = InputEvent::TransferResponse { request, paths };
        let input = if input.is_valid() {
            input
        } else {
            InputEvent::TransferResponse {
                request,
                paths: Vec::new(),
            }
        };
        let _ = live.send(Command::Input { document, input });
    }

    pub(super) fn cancel_download(&mut self, request: u64, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            let _ = live.send_to_document(|document| Command::Input {
                document,
                input: InputEvent::CancelDownload { request },
            });
        }
        cx.notify();
    }
}
