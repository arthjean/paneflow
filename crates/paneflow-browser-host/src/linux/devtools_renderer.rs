use std::cell::RefCell;
use std::collections::BTreeSet;

use cef::*;

use super::devtools::{MARKER, MAX_MESSAGE, MESSAGE, URL};

thread_local! {
    static INSPECTORS: RefCell<BTreeSet<i32>> = const { RefCell::new(BTreeSet::new()) };
}

pub(super) fn created(browser: &Browser, info: Option<&DictionaryValue>) {
    if info.is_some_and(|info| info.bool(Some(&MARKER.into())) != 0) {
        INSPECTORS.with(|state| state.borrow_mut().insert(browser.identifier()));
    }
}

pub(super) fn destroyed(browser: &Browser) {
    INSPECTORS.with(|state| state.borrow_mut().remove(&browser.identifier()));
}

pub(super) fn context(browser: &Browser, frame: &Frame, context: &V8Context) {
    if !INSPECTORS.with(|state| state.borrow().contains(&browser.identifier()))
        || frame.is_main() == 0
        || CefString::from(&frame.url()).to_string() != URL
    {
        return;
    }
    let Some(global) = context.global() else {
        return;
    };
    let Some(mut function) = v8_value_create_function(
        Some(&"paneflowDevToolsSend".into()),
        Some(&mut Send::new(frame.clone())),
    ) else {
        return;
    };
    global.set_value_bykey(
        Some(&"paneflowDevToolsSend".into()),
        Some(&mut function),
        V8Propertyattribute::default(),
    );
    frame.execute_java_script(
        Some(
            &r#"(() => {
        let frontend;
        Object.defineProperty(window, 'InspectorFrontendHost', {
            configurable: false,
            get: () => frontend,
            set: value => {
                value.sendMessageToBackend = message => window.paneflowDevToolsSend(message);
                value.isHostedMode = () => false;
                if (frontend !== value) {
                    const getPreferences = value.getPreferences?.bind(value);
                    value.getPreferences = callback => {
                        const apply = preferences => callback({'screencast-enabled': 'false', ...preferences});
                        if (getPreferences) getPreferences(apply); else apply({});
                    };
                }
                frontend = value;
            }
        });
    })()"#
                .into(),
        ),
        Some(&URL.into()),
        0,
    );
}

wrap_v8_handler! {
    struct Send { frame: Frame }

    impl V8Handler {
        fn execute(&self, _name: Option<&CefString>, _object: Option<&mut V8Value>, arguments: Option<&[Option<V8Value>]>, _retval: Option<&mut Option<V8Value>>, _exception: Option<&mut CefString>) -> i32 {
            if self.frame.is_main() == 0 || CefString::from(&self.frame.url()).to_string() != URL { return 1; }
            let Some(value) = arguments.filter(|args| args.len() == 1).and_then(|args| args[0].as_ref()).filter(|value| value.is_string() != 0) else { return 1; };
            let payload = CefString::from(&value.string_value()).to_string();
            if payload.len() > MAX_MESSAGE { return 1; }
            let Some(mut message) = process_message_create(Some(&MESSAGE.into())) else { return 1; };
            if let Some(args) = message.argument_list() { args.set_string(0, Some(&payload.as_str().into())); self.frame.send_process_message(ProcessId::BROWSER, Some(&mut message)); }
            1
        }
    }
}
