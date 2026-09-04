use gpui::{App, AppContext, Entity, Global};

#[derive(Default)]
pub struct ThemeSignal {
    generation: u64,
}

impl ThemeSignal {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

pub struct ThemeSignalGlobal(pub Entity<ThemeSignal>);

impl Global for ThemeSignalGlobal {}

pub fn install_theme_signal(cx: &mut App) {
    if cx.has_global::<ThemeSignalGlobal>() {
        return;
    }
    let signal = cx.new(|_| ThemeSignal {
        generation: super::theme_generation(),
    });
    cx.set_global(ThemeSignalGlobal(signal));
}

pub fn theme_signal(cx: &App) -> Option<Entity<ThemeSignal>> {
    cx.try_global::<ThemeSignalGlobal>()
        .map(|global| global.0.clone())
}

pub fn publish_theme_generation(cx: &mut App) {
    let generation = super::theme_generation();
    let Some(signal) = theme_signal(cx) else {
        return;
    };
    signal.update(cx, |signal, cx| {
        if signal.generation == generation {
            return;
        }
        signal.generation = generation;
        cx.notify();
    });
}
