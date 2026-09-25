use super::*;

pub(super) struct TranscriptTurnEndNotification {
    pub(super) agent: TerminalAgent,
    pub(super) title: String,
    pub(super) config: PaneFlowConfig,
    pub(super) seen: bool,
    pub(super) executor: BackgroundExecutor,
}

fn read_last_result(params: &serde_json::Value) -> Option<String> {
    let hook = params.get("hook_payload");
    let raw = ["last_result", "summary", "result"].iter().find_map(|k| {
        params
            .get(*k)
            .or_else(|| hook.and_then(|h| h.get(*k)))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    })?;
    Some(crate::markdown::strip_bidi_zero_width(
        raw.chars().take(2048).collect(),
    ))
}

const TRANSCRIPT_READ_CAP: u64 = 4 * 1024 * 1024;

pub(super) fn read_transcript_path(params: &serde_json::Value) -> Option<std::path::PathBuf> {
    let hook = params.get("hook_payload");
    let raw = params
        .get("transcript_path")
        .or_else(|| hook.and_then(|h| h.get("transcript_path")))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let path = std::path::PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

fn extract_last_result_from_transcript(path: &std::path::Path) -> Option<String> {
    extract_last_result_capped(path, TRANSCRIPT_READ_CAP)
}

pub(super) fn read_stop_summary(
    params: &serde_json::Value,
) -> (Option<String>, Option<std::path::PathBuf>) {
    let inline = read_last_result(params);
    let transcript_path = inline
        .is_none()
        .then(|| read_transcript_path(params))
        .flatten();
    (inline, transcript_path)
}

fn extract_last_result_capped(path: &std::path::Path, cap: u64) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    let mut content = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_string(&mut content)
        .ok()?;
    for line in content.rsplit('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        let text = blocks
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if text.trim().is_empty() {
            continue;
        }
        return Some(crate::markdown::strip_bidi_zero_width(
            text.chars().take(2048).collect(),
        ));
    }
    None
}

impl PaneFlowApp {
    pub(super) fn schedule_transcript_turn_end(
        update_target: Option<(u64, u32)>,
        path: std::path::PathBuf,
        notification: Option<TranscriptTurnEndNotification>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let extracted =
                    smol::unblock(move || extract_last_result_from_transcript(&path)).await;
                if let Some(notification) = notification {
                    desktop_notifications::fire_desktop_notification(
                        DesktopNotification::turn_finished(
                            notification.agent,
                            &notification.title,
                            extracted.as_deref(),
                        ),
                        &notification.config,
                        notification.seen,
                        notification.executor,
                    );
                }
                let (Some((ws_id, session_key)), Some(text)) = (update_target, extracted) else {
                    return;
                };
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.record_auto_naming_message(
                            ws_id,
                            session_key,
                            crate::auto_naming::Role::Assistant,
                            &text,
                        );
                        app.schedule_auto_naming(ws_id, session_key, cx);
                        let filled = if let Some(ws) =
                            app.workspaces.iter_mut().find(|ws| ws.id == ws_id)
                            && let Some(s) = ws.agent_sessions.get_mut(&session_key)
                            && s.last_result.is_none()
                        {
                            s.last_result = Some(text);
                            true
                        } else {
                            false
                        };
                        if filled {
                            app.agent_sessions_changed(cx);
                            cx.notify();
                        }
                    });
                });
            },
        )
        .detach();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn read_last_result_best_effort_or_none() {
        let p = serde_json::json!({"hook_payload": {"summary": "wrote 3 files"}});
        assert_eq!(
            super::read_last_result(&p).as_deref(),
            Some("wrote 3 files")
        );
        let p = serde_json::json!({"last_result": "done"});
        assert_eq!(super::read_last_result(&p).as_deref(), Some("done"));
        let p = serde_json::json!({"hook_payload": {"transcript_path": "/tmp/x.jsonl"}});
        assert!(super::read_last_result(&p).is_none());
        assert!(super::read_last_result(&serde_json::json!({})).is_none());
    }

    #[test]
    fn read_transcript_path_absolute_only() {
        use super::read_transcript_path;
        #[cfg(windows)]
        let (abs_a, abs_b) = (r"C:\abs\a.jsonl", r"C:\abs\b.jsonl");
        #[cfg(not(windows))]
        let (abs_a, abs_b) = ("/abs/a.jsonl", "/abs/b.jsonl");
        let p = serde_json::json!({ "transcript_path": abs_a });
        assert_eq!(
            read_transcript_path(&p).as_deref(),
            Some(std::path::Path::new(abs_a))
        );
        let p = serde_json::json!({ "hook_payload": { "transcript_path": abs_b } });
        assert_eq!(
            read_transcript_path(&p).as_deref(),
            Some(std::path::Path::new(abs_b))
        );
        assert!(
            read_transcript_path(&serde_json::json!({"transcript_path": "rel/x.jsonl"})).is_none()
        );
        assert!(
            read_transcript_path(&serde_json::json!({"hook_payload": {"transcript_path": ""}}))
                .is_none()
        );
        assert!(read_transcript_path(&serde_json::json!({})).is_none());
    }

    #[test]
    fn read_stop_summary_uses_inline_before_transcript_path() {
        #[cfg(windows)]
        let abs = r"C:\abs\session.jsonl";
        #[cfg(not(windows))]
        let abs = "/abs/session.jsonl";

        let p = serde_json::json!({"hook_payload": {"summary": "done", "transcript_path": abs}});
        let (summary, path) = super::read_stop_summary(&p);
        assert_eq!(summary.as_deref(), Some("done"));
        assert!(path.is_none());

        let p = serde_json::json!({"hook_payload": {"transcript_path": abs}});
        let (summary, path) = super::read_stop_summary(&p);
        assert!(summary.is_none());
        assert_eq!(path.as_deref(), Some(std::path::Path::new(abs)));
    }

    #[test]
    fn transcript_extracts_last_outermost_assistant_text() {
        use super::extract_last_result_from_transcript;
        let jsonl = concat!(
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{"type":"assistant","isSidechain":false,"message":{"content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"First answer."}]}}"#,
            "\n",
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"SUBAGENT noise"}]}}"#,
            "\n",
            r#"{"type":"assistant","isSidechain":false,"message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":{}}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","stop_reason":"end_turn"}"#,
            "\n",
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, jsonl).expect("write fixture");
        assert_eq!(
            extract_last_result_from_transcript(&path).as_deref(),
            Some("First answer.")
        );
    }

    #[test]
    fn transcript_absent_or_oversize_or_textless_is_none() {
        use super::{extract_last_result_capped, extract_last_result_from_transcript};
        assert!(
            extract_last_result_from_transcript(std::path::Path::new("/no/such/transcript.jsonl"))
                .is_none()
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let big = dir.path().join("big.jsonl");
        std::fs::write(&big, "x".repeat(64)).expect("write");
        assert!(extract_last_result_capped(&big, 10).is_none());
        let none = dir.path().join("none.jsonl");
        std::fs::write(
            &none,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":{}}]}}"#,
                "\n",
            ),
        )
        .expect("write");
        assert!(extract_last_result_from_transcript(&none).is_none());
    }
}
