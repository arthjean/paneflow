use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const STANDARD_TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];

pub const RUST_ANALYZER_TOKEN_TYPES: &[&str] = &[
    "selfKeyword",
    "builtinType",
    "procMacro",
    "formatSpecifier",
    "lifetime",
    "macroBang",
    "unresolvedReference",
];

pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

pub const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

impl PositionEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
        }
    }

    fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("utf-16") => Some(Self::Utf16),
            Some("utf-8") => Some(Self::Utf8),
            Some(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncKind {
    None,
    Full,
    Incremental,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSync {
    pub open_close: bool,
    pub change: SyncKind,
}

impl TextSync {
    pub fn offers_sync(&self) -> bool {
        self.open_close || self.change != SyncKind::None
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenLegend {
    pub token_types: Vec<String>,
    pub token_modifiers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticTokensSupport {
    pub legend: TokenLegend,
    pub full: bool,
    pub delta: bool,
    pub range: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiated {
    pub encoding: PositionEncoding,
    pub sync: TextSync,
    pub semantic_tokens: Option<SemanticTokensSupport>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapabilityError {
    UnsupportedEncoding(String),
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedEncoding(encoding) => {
                write!(
                    f,
                    "the server chose position encoding {encoding:?}, which was not offered"
                )
            }
        }
    }
}

impl Negotiated {
    pub fn from_initialize_result(result: &Value) -> Result<Self, CapabilityError> {
        let capabilities = result.get("capabilities").unwrap_or(&Value::Null);
        let requested = capabilities.get("positionEncoding").and_then(Value::as_str);
        let encoding = PositionEncoding::parse(requested).ok_or_else(|| {
            CapabilityError::UnsupportedEncoding(requested.unwrap_or_default().to_string())
        })?;
        Ok(Self {
            encoding,
            sync: parse_text_sync(capabilities.get("textDocumentSync")),
            semantic_tokens: parse_semantic_tokens(capabilities.get("semanticTokensProvider")),
        })
    }
}

fn sync_kind(value: Option<&Value>) -> SyncKind {
    match value.and_then(Value::as_u64) {
        Some(1) => SyncKind::Full,
        Some(2) => SyncKind::Incremental,
        _ => SyncKind::None,
    }
}

fn parse_text_sync(value: Option<&Value>) -> TextSync {
    match value {
        Some(Value::Number(_)) => {
            let change = sync_kind(value);
            TextSync {
                open_close: change != SyncKind::None,
                change,
            }
        }
        Some(Value::Object(options)) => TextSync {
            open_close: options
                .get("openClose")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            change: sync_kind(options.get("change")),
        },
        _ => TextSync {
            open_close: false,
            change: SyncKind::None,
        },
    }
}

fn enabled(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

fn parse_semantic_tokens(value: Option<&Value>) -> Option<SemanticTokensSupport> {
    let provider = value?.as_object()?;
    let legend: TokenLegend = serde_json::from_value(provider.get("legend")?.clone()).ok()?;
    let full = provider.get("full");
    Some(SemanticTokensSupport {
        legend,
        full: enabled(full),
        delta: full
            .and_then(|full| full.get("delta"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        range: enabled(provider.get("range")),
    })
}

pub fn client_capabilities() -> Value {
    let token_types: Vec<&str> = STANDARD_TOKEN_TYPES
        .iter()
        .chain(RUST_ANALYZER_TOKEN_TYPES)
        .copied()
        .collect();
    json!({
        "general": {
            "positionEncodings": ["utf-8", "utf-16"]
        },
        "workspace": {
            "configuration": true,
            "workspaceFolders": true,
            "semanticTokens": {
                "refreshSupport": true
            }
        },
        "window": {
            "workDoneProgress": true
        },
        "textDocument": {
            "synchronization": {
                "dynamicRegistration": false,
                "didSave": false,
                "willSave": false,
                "willSaveWaitUntil": false
            },
            "semanticTokens": {
                "dynamicRegistration": false,
                "requests": {
                    "range": true,
                    "full": {
                        "delta": true
                    }
                },
                "tokenTypes": token_types,
                "tokenModifiers": TOKEN_MODIFIERS,
                "formats": ["relative"],
                "overlappingTokenSupport": false,
                "multilineTokenSupport": false,
                "serverCancelSupport": true,
                "augmentsSyntaxTokens": true
            }
        }
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFolder {
    pub uri: String,
    pub name: String,
}

pub fn initialize_params(
    root_uri: &str,
    root_name: &str,
    initialization_options: Option<&Value>,
) -> Value {
    let mut params = json!({
        "processId": std::process::id(),
        "clientInfo": {
            "name": "Paneflow",
            "version": env!("CARGO_PKG_VERSION")
        },
        "rootUri": root_uri,
        "workspaceFolders": [WorkspaceFolder {
            uri: root_uri.to_string(),
            name: root_name.to_string(),
        }],
        "capabilities": client_capabilities(),
        "trace": "off"
    });
    if let (Some(options), Some(object)) = (initialization_options, params.as_object_mut()) {
        object.insert("initializationOptions".to_string(), options.clone());
    }
    params
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationItem {
    #[serde(default, rename = "scopeUri", skip_serializing_if = "Option::is_none")]
    pub scope_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationParams {
    pub items: Vec<ConfigurationItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogMessageParams {
    #[serde(rename = "type")]
    pub kind: i64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelParams {
    pub id: Value,
}

pub fn configuration_answer(settings: &Value, params: &ConfigurationParams) -> Value {
    Value::Array(
        params
            .items
            .iter()
            .map(|item| match item.section.as_deref() {
                Some(section) => settings_section(settings, section)
                    .unwrap_or(settings)
                    .clone(),
                None => settings.clone(),
            })
            .collect(),
    )
}

fn settings_section<'a>(settings: &'a Value, section: &str) -> Option<&'a Value> {
    if let Some(value) = settings.get(section) {
        return Some(value);
    }
    section
        .split('.')
        .try_fold(settings, |value, part| value.get(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_examples_round_trip_through_the_message_shapes() {
        let open = json!({"textDocument": {"uri": "file:///a.rs", "languageId": "rust", "version": 1, "text": "fn main() {}"}});
        let parsed: DidOpenTextDocumentParams = serde_json::from_value(open.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), open);

        let change = json!({
            "textDocument": {"uri": "file:///a.rs", "version": 2},
            "contentChanges": [
                {"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}}, "text": "x"},
                {"text": "whole"}
            ]
        });
        let parsed: DidChangeTextDocumentParams = serde_json::from_value(change.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), change);

        let close = json!({"textDocument": {"uri": "file:///a.rs"}});
        let parsed: DidCloseTextDocumentParams = serde_json::from_value(close.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), close);

        let configuration =
            json!({"items": [{"scopeUri": "file:///r", "section": "rust-analyzer"}, {}]});
        let parsed: ConfigurationParams = serde_json::from_value(configuration.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), configuration);

        let log = json!({"type": 3, "message": "hello"});
        let parsed: LogMessageParams = serde_json::from_value(log.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), log);

        let cancel = json!({"id": 7});
        let parsed: CancelParams = serde_json::from_value(cancel.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), cancel);
    }

    #[test]
    fn initialize_carries_the_root_one_folder_and_the_semantic_token_capabilities() {
        let params = initialize_params("file:///repo", "repo", Some(&json!({"cargo": {}})));
        assert_eq!(params["rootUri"], "file:///repo");
        assert_eq!(
            params["workspaceFolders"],
            json!([{"uri": "file:///repo", "name": "repo"}])
        );
        assert_eq!(params["initializationOptions"], json!({"cargo": {}}));
        let tokens = &params["capabilities"]["textDocument"]["semanticTokens"];
        assert_eq!(tokens["formats"], json!(["relative"]));
        assert_eq!(tokens["augmentsSyntaxTokens"], json!(true));
        assert_eq!(tokens["multilineTokenSupport"], json!(false));
        assert_eq!(tokens["overlappingTokenSupport"], json!(false));
        assert_eq!(
            tokens["requests"],
            json!({"range": true, "full": {"delta": true}})
        );
        assert!(tokens["tokenTypes"]
            .as_array()
            .unwrap()
            .contains(&json!("selfKeyword")));
        assert!(tokens["tokenModifiers"]
            .as_array()
            .unwrap()
            .contains(&json!("defaultLibrary")));
        assert_eq!(
            params["capabilities"]["workspace"]["semanticTokens"]["refreshSupport"],
            json!(true)
        );
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"],
            json!(["utf-8", "utf-16"])
        );
    }

    #[test]
    fn the_initialize_result_gives_the_encoding_the_sync_kind_and_the_legend() {
        let result = json!({"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2},
            "semanticTokensProvider": {
                "legend": {"tokenTypes": ["function"], "tokenModifiers": ["static"]},
                "full": {"delta": true},
                "range": true
            }
        }});
        let negotiated = Negotiated::from_initialize_result(&result).unwrap();
        assert_eq!(negotiated.encoding, PositionEncoding::Utf8);
        assert_eq!(
            negotiated.sync,
            TextSync {
                open_close: true,
                change: SyncKind::Incremental
            }
        );
        let tokens = negotiated.semantic_tokens.unwrap();
        assert_eq!(tokens.legend.token_types, vec!["function".to_string()]);
        assert!(tokens.full && tokens.delta && tokens.range);

        let bare =
            Negotiated::from_initialize_result(&json!({"capabilities": {"textDocumentSync": 1}}))
                .unwrap();
        assert_eq!(bare.encoding, PositionEncoding::Utf16);
        assert_eq!(bare.sync.change, SyncKind::Full);
        assert!(bare.sync.open_close);
        assert!(bare.semantic_tokens.is_none());

        let none = Negotiated::from_initialize_result(&json!({"capabilities": {}})).unwrap();
        assert!(!none.sync.offers_sync());

        assert!(Negotiated::from_initialize_result(
            &json!({"capabilities": {"positionEncoding": "utf-32"}})
        )
        .is_err());
    }

    #[test]
    fn configuration_answers_a_named_section_or_the_whole_settings_object() {
        let settings =
            json!({"semanticTokens": true, "rust-analyzer": {"cargo": {"features": "all"}}});
        let params = ConfigurationParams {
            items: vec![
                ConfigurationItem {
                    scope_uri: None,
                    section: Some("rust-analyzer".into()),
                },
                ConfigurationItem {
                    scope_uri: None,
                    section: Some("gopls".into()),
                },
                ConfigurationItem {
                    scope_uri: None,
                    section: Some("rust-analyzer.cargo".into()),
                },
                ConfigurationItem {
                    scope_uri: None,
                    section: None,
                },
            ],
        };
        assert_eq!(
            configuration_answer(&settings, &params),
            json!([
                {"cargo": {"features": "all"}},
                settings.clone(),
                {"features": "all"},
                settings.clone()
            ])
        );
    }
}
