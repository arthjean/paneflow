use std::ops::Range;
use std::path::Path;

use wasmtime::{
    Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, TypedFunc,
};

pub(crate) const HIGHLIGHTS_WASM_ENV: &str = "PANEFLOW_HIGHLIGHTS_WASM";

pub(crate) const PLAIN_LANGUAGE: u8 = 0;

const RUN_EVERYTHING: i32 = i32::MAX;
const PENDING_LINES_STAT: i32 = 9;
const WIDE_RECORDS_FLAG: i32 = 4;
const PACKED_END_MASK: u32 = 0x00ff_ffff;
const EDIT_HEADER_BYTES: usize = 4;
const EDIT_RECORD_BYTES: usize = 24;

pub(crate) const TOKEN_NAMES: [&str; 73] = [
    "none",
    "attribute",
    "attribute.jsx",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "diff.minus",
    "diff.plus",
    "embedded",
    "emphasis",
    "emphasis.strong",
    "function",
    "function.definition",
    "function.method",
    "keyword",
    "keyword.control",
    "keyword.declaration",
    "keyword.import",
    "keyword.operator",
    "label",
    "namespace",
    "number",
    "operator",
    "predictive",
    "preproc",
    "primary",
    "property",
    "property.json_key",
    "punctuation",
    "punctuation.bracket",
    "punctuation.bracket.html",
    "punctuation.bracket.jsx",
    "punctuation.delimiter",
    "punctuation.delimiter.html",
    "punctuation.delimiter.jsx",
    "punctuation.list_marker",
    "punctuation.markup",
    "punctuation.special",
    "selector",
    "selector.class",
    "selector.id",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "tag.doctype",
    "tag.jsx",
    "text.jsx",
    "text.literal",
    "title",
    "type",
    "type.builtin",
    "type.class",
    "variable",
    "variable.parameter",
    "variable.special",
    "enum",
    "hint",
    "keyword.jsdoc",
    "link_text",
    "link_uri",
    "selector.pseudo",
    "tag.component.jsx",
    "type.jsdoc",
    "variable.jsdoc",
    "variant",
    "background",
    "foreground",
];

const LANGUAGES: &[(&str, u8)] = &[
    ("plain", 0),
    ("text", 0),
    ("bash", 11),
    ("sh", 11),
    ("c", 16),
    ("css", 27),
    ("go", 52),
    ("html", 62),
    ("javascript", 67),
    ("js", 67),
    ("json", 68),
    ("markdown", 87),
    ("md", 87),
    ("py", 112),
    ("python", 112),
    ("rs", 120),
    ("rust", 120),
    ("toml", 135),
    ("ts", 136),
    ("typescript", 136),
    ("tsx", 139),
    ("yaml", 151),
];

pub(crate) fn language_id(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, id)| *id)
}

pub(crate) fn token_name(category: u8) -> &'static str {
    TOKEN_NAMES
        .get(usize::from(category))
        .copied()
        .unwrap_or("none")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LexToken {
    pub(crate) range: Range<usize>,
    pub(crate) category: u8,
}

pub(crate) struct LexerPrototype {
    engine: Engine,
    module: Module,
    wasm_bytes: usize,
}

fn error(context: &str, err: impl std::fmt::Display) -> String {
    format!("{context}: {err}")
}

fn decode_code_point(bytes: &[u8]) -> Option<char> {
    std::str::from_utf8(bytes).ok()?.chars().next()
}

fn code_point_at(caller: &mut Caller<'_, ()>, ptr: i32, len: i32) -> Option<char> {
    let memory = caller.get_export("memory").and_then(Extern::into_memory)?;
    let start = usize::try_from(ptr).ok()?;
    let len = usize::try_from(len).ok()?;
    decode_code_point(memory.data(&caller).get(start..start.checked_add(len)?)?)
}

pub(crate) fn is_id_start(ch: char) -> bool {
    unicode_ident::is_xid_start(ch)
}

pub(crate) fn is_id_continue(ch: char) -> bool {
    unicode_ident::is_xid_continue(ch) || matches!(ch, '\u{200C}' | '\u{200D}')
}

impl LexerPrototype {
    pub(crate) fn from_env() -> Result<Self, String> {
        let path = std::env::var_os(HIGHLIGHTS_WASM_ENV)
            .ok_or_else(|| format!("{HIGHLIGHTS_WASM_ENV} is not set"))?;
        Self::load(Path::new(&path))
    }

    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|err| error(&path.display().to_string(), err))?;
        Self::from_bytes(&bytes)
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::new(&Config::new()).map_err(|err| error("engine", err))?;
        let module = Module::new(&engine, bytes).map_err(|err| error("compile", err))?;
        Ok(Self {
            engine,
            module,
            wasm_bytes: bytes.len(),
        })
    }

    pub(crate) fn wasm_bytes(&self) -> usize {
        self.wasm_bytes
    }

    fn instantiate(&self) -> Result<(Store<()>, Instance), String> {
        let mut linker = Linker::new(&self.engine);
        linker
            .func_wrap(
                "env",
                "is_id_start",
                |mut caller: Caller<'_, ()>, ptr: i32, len: i32| -> i32 {
                    i32::from(code_point_at(&mut caller, ptr, len).is_some_and(is_id_start))
                },
            )
            .map_err(|err| error("is_id_start", err))?;
        linker
            .func_wrap(
                "env",
                "is_id_continue",
                |mut caller: Caller<'_, ()>, ptr: i32, len: i32| -> i32 {
                    i32::from(code_point_at(&mut caller, ptr, len).is_some_and(is_id_continue))
                },
            )
            .map_err(|err| error("is_id_continue", err))?;
        let mut store = Store::new(&self.engine, ());
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|err| error("instantiate", err))?;
        Ok((store, instance))
    }

    pub(crate) fn document(&self, text: &str, language: Option<u8>) -> Result<LexDocument, String> {
        let (mut store, instance) = self.instantiate()?;
        let exports = Exports::resolve(&mut store, &instance)?;
        let language = language.unwrap_or(PLAIN_LANGUAGE);
        let mut document = LexDocument {
            store,
            exports,
            language,
        };
        let ptr = document.stage(text.as_bytes())?;
        let len = document.len_i32(text.len())?;
        document
            .exports
            .init_doc
            .call(&mut document.store, (ptr, len, i32::from(language)))
            .map_err(|err| error("liveInitDoc", err))?;
        document.settle()?;
        Ok(document)
    }
}

struct Exports {
    memory: Memory,
    stage: TypedFunc<i32, i32>,
    init_doc: TypedFunc<(i32, i32, i32), ()>,
    apply_edits: TypedFunc<(i32, i32), ()>,
    run: TypedFunc<(i32, i32, i32), i32>,
    line_count: TypedFunc<(), i32>,
    line_flags: TypedFunc<i32, i32>,
    tok_ptr: TypedFunc<i32, i32>,
    tok_count: TypedFunc<i32, i32>,
    stats: TypedFunc<i32, i32>,
}

impl Exports {
    fn resolve(store: &mut Store<()>, instance: &Instance) -> Result<Self, String> {
        let memory = instance
            .get_memory(&mut *store, "memory")
            .ok_or_else(|| "the module exports no memory".to_owned())?;
        macro_rules! typed {
            ($name:literal) => {
                instance
                    .get_typed_func(&mut *store, $name)
                    .map_err(|err| error($name, err))?
            };
        }
        Ok(Self {
            memory,
            stage: typed!("liveStage"),
            init_doc: typed!("liveInitDoc"),
            apply_edits: typed!("liveApplyEdits"),
            run: typed!("liveRun"),
            line_count: typed!("liveLineCount"),
            line_flags: typed!("liveLineFlags"),
            tok_ptr: typed!("liveLineTokPtr"),
            tok_count: typed!("liveLineTokCount"),
            stats: typed!("liveStats"),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LexPosition {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

pub(crate) struct LexDocument {
    store: Store<()>,
    exports: Exports,
    language: u8,
}

fn to_i32(value: usize) -> Result<i32, String> {
    i32::try_from(value).map_err(|_| format!("{value} does not fit the module's 32-bit offsets"))
}

fn to_usize(value: i32) -> usize {
    usize::try_from(value).unwrap_or(0)
}

impl LexDocument {
    fn len_i32(&self, len: usize) -> Result<i32, String> {
        to_i32(len)
    }

    fn stage(&mut self, bytes: &[u8]) -> Result<i32, String> {
        let len = to_i32(bytes.len())?;
        let ptr = self
            .exports
            .stage
            .call(&mut self.store, len)
            .map_err(|err| error("liveStage", err))?;
        let start = to_usize(ptr);
        let memory = self.exports.memory.data_mut(&mut self.store);
        memory
            .get_mut(start..start + bytes.len())
            .ok_or_else(|| "liveStage returned a block past the memory".to_owned())?
            .copy_from_slice(bytes);
        Ok(ptr)
    }

    pub(crate) fn settle(&mut self) -> Result<(), String> {
        self.exports
            .run
            .call(&mut self.store, (RUN_EVERYTHING, 0, 0))
            .map_err(|err| error("liveRun", err))?;
        Ok(())
    }

    pub(crate) fn pending_lines(&mut self) -> usize {
        to_usize(
            self.exports
                .stats
                .call(&mut self.store, PENDING_LINES_STAT)
                .unwrap_or_default(),
        )
    }

    pub(crate) fn language(&self) -> u8 {
        self.language
    }

    pub(crate) fn line_count(&mut self) -> usize {
        to_usize(
            self.exports
                .line_count
                .call(&mut self.store, ())
                .unwrap_or_default(),
        )
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        self.exports.memory.data_size(&self.store)
    }

    pub(crate) fn replace(
        &mut self,
        start: LexPosition,
        end: LexPosition,
        text: &str,
    ) -> Result<(), String> {
        let mut block = vec![0u8; EDIT_HEADER_BYTES + EDIT_RECORD_BYTES + text.len()];
        let fields = [
            1u32,
            to_i32(start.line)? as u32,
            to_i32(start.column)? as u32,
            to_i32(end.line)? as u32,
            to_i32(end.column)? as u32,
            (EDIT_HEADER_BYTES + EDIT_RECORD_BYTES) as u32,
            to_i32(text.len())? as u32,
        ];
        for (index, field) in fields.iter().enumerate() {
            block[index * 4..index * 4 + 4].copy_from_slice(&field.to_le_bytes());
        }
        block[EDIT_HEADER_BYTES + EDIT_RECORD_BYTES..].copy_from_slice(text.as_bytes());
        let ptr = self.stage(&block)?;
        self.exports
            .apply_edits
            .call(&mut self.store, (ptr, 0))
            .map_err(|err| error("liveApplyEdits", err))?;
        self.settle()
    }

    pub(crate) fn line_tokens(&mut self, line: usize) -> Vec<LexToken> {
        let Ok(line) = to_i32(line) else {
            return Vec::new();
        };
        let (count, ptr, flags) = {
            let exports = &self.exports;
            let count = exports
                .tok_count
                .call(&mut self.store, line)
                .unwrap_or_default();
            let ptr = exports
                .tok_ptr
                .call(&mut self.store, line)
                .unwrap_or_default();
            let flags = exports
                .line_flags
                .call(&mut self.store, line)
                .unwrap_or_default();
            (to_usize(count), to_usize(ptr), flags)
        };
        let wide = flags & WIDE_RECORDS_FLAG != 0;
        let memory = self.exports.memory.data(&self.store);
        let word = |index: usize| -> u32 {
            let at = ptr + index * 4;
            memory
                .get(at..at + 4)
                .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                .unwrap_or_default()
        };
        let mut tokens = Vec::with_capacity(count);
        let mut start = 0usize;
        for record in 0..count {
            let (end, category) = if wide {
                (word(record * 2) as usize, word(record * 2 + 1))
            } else {
                let packed = word(record);
                ((packed & PACKED_END_MASK) as usize, packed >> 24)
            };
            if end > start {
                tokens.push(LexToken {
                    range: start..end,
                    category: u8::try_from(category).unwrap_or(0),
                });
                start = end;
            }
        }
        tokens
    }

    pub(crate) fn query(&mut self, lines: Range<usize>) -> Vec<Vec<LexToken>> {
        let end = lines.end.min(self.line_count());
        (lines.start.min(end)..end)
            .map(|line| self.line_tokens(line))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> LexerPrototype {
        LexerPrototype::from_env().unwrap_or_else(|reason| {
            panic!("the lexer prototype tests need {HIGHLIGHTS_WASM_ENV}: {reason}")
        })
    }

    fn names(tokens: &[LexToken], line: &str) -> Vec<(String, &'static str)> {
        tokens
            .iter()
            .map(|token| {
                (
                    line[token.range.clone()].to_owned(),
                    token_name(token.category),
                )
            })
            .collect()
    }

    #[test]
    fn the_host_functions_answer_the_unicode_identifier_classes() {
        assert!(is_id_start('a') && is_id_start('é') && is_id_start('字'));
        assert!(!is_id_start('1') && !is_id_start('-'));
        assert!(is_id_continue('1') && is_id_continue('_'));
        assert!(is_id_continue('\u{200D}') && is_id_continue('\u{200C}'));
        assert_eq!(decode_code_point("é".as_bytes()), Some('é'));
        assert_eq!(decode_code_point(&[0xff]), None);
    }

    #[test]
    fn a_language_resolves_by_name_or_extension_and_an_unknown_one_does_not() {
        assert_eq!(language_id("RS"), Some(120));
        assert_eq!(language_id("typescript"), Some(136));
        assert_eq!(language_id("cobol"), None);
        assert_eq!(token_name(17), "keyword");
        assert_eq!(token_name(250), "none");
    }

    #[test]
    fn an_unknown_language_comes_back_as_uncolored_text() {
        let engine = engine();
        let text = "IDENTIFICATION DIVISION.\nPROGRAM-ID. HELLO.\n";
        let mut doc = engine
            .document(text, language_id("cobol"))
            .expect("a plain document");
        assert_eq!(doc.language(), PLAIN_LANGUAGE);
        let lines = doc.query(0..10);
        assert_eq!(lines.len(), 3);
        for (tokens, line) in lines.iter().zip(text.lines()) {
            assert!(tokens.iter().all(|token| token.category == 0), "{tokens:?}");
            assert_eq!(tokens.last().map_or(0, |token| token.range.end), line.len());
        }
    }

    #[test]
    fn rust_is_tokenized_and_an_edit_retokenizes_the_line() {
        let engine = engine();
        let text = "fn main() {\n    let total = 42;\n}\n";
        let mut doc = engine
            .document(text, language_id("rs"))
            .expect("a document");
        assert_eq!(doc.pending_lines(), 0);
        let first = names(&doc.line_tokens(0), "fn main() {");
        assert_eq!(first[0].0.trim(), "fn", "{first:?}");
        assert!(first[0].1.starts_with("keyword"), "{first:?}");
        assert!(
            first
                .iter()
                .any(|(text, name)| text == "main" && name.starts_with("function"))
        );
        let second = names(&doc.line_tokens(1), "    let total = 42;");
        assert!(
            second
                .iter()
                .any(|(text, name)| text == "42" && *name == "number"),
            "{second:?}"
        );

        let at = LexPosition { line: 1, column: 4 };
        doc.replace(at, at, "// ").expect("an edit");
        let edited = names(&doc.line_tokens(1), "    // let total = 42;");
        assert!(
            edited
                .iter()
                .any(|(text, name)| text.contains("let total") && name.starts_with("comment")),
            "{edited:?}"
        );
        assert_eq!(doc.query(2..99).len(), 2);
    }

    #[test]
    fn a_unicode_identifier_goes_through_the_host_functions() {
        let engine = engine();
        let line = "let café = 1;";
        let mut doc = engine
            .document(line, language_id("rs"))
            .expect("a document");
        let tokens = doc.line_tokens(0);
        let utf16 = line.encode_utf16().count();
        assert_eq!(tokens.last().map(|token| token.range.end), Some(utf16));
        assert!(doc.memory_bytes() > 0);
        assert!(engine.wasm_bytes() > 0);
    }
}
