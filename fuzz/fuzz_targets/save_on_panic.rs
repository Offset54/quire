//! On Windows a panic in a fuzz target ends the process with a fast fail
//! (0xc0000409) before libFuzzer can write the crashing input. Targets call
//! [`guard`] with each input: a panic first writes it to
//! `artifacts/<target>/crash-panic` (in the working directory).

use std::cell::RefCell;
use std::sync::Once;

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Remembers `data` as the current input; installs the hook once.
pub fn guard(target: &'static str, data: &[u8]) {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let dir = std::path::Path::new("artifacts").join(target);
            let _ = std::fs::create_dir_all(&dir);
            INPUT.with(|i| {
                let _ = std::fs::write(dir.join("crash-panic"), &*i.borrow());
            });
            previous(info);
        }));
    });
    INPUT.with(|i| {
        let mut i = i.borrow_mut();
        i.clear();
        i.extend_from_slice(data);
    });
}
