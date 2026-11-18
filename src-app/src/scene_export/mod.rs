mod agents_window;
mod changes_window;
mod export;
mod fixture;
mod spy;
mod vt;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use gpui::{DevicePixels, HeadlessAppContext, PlatformTextSystem, px, size};

pub(crate) const WINDOW_WIDTH: f32 = 1893.;
pub(crate) const WINDOW_HEIGHT: f32 = 1165.;
pub(crate) const FRAME_INSET: f32 = 10.;

fn out_dir() -> Result<PathBuf> {
    let dir = std::env::var_os("PANEFLOW_SCENE_OUT")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("set PANEFLOW_SCENE_OUT to the export directory"))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn isolate_home(out: &std::path::Path) -> Result<()> {
    let home = out.join("home");
    if home.exists() {
        std::fs::remove_dir_all(&home)?;
    }
    std::fs::create_dir_all(&home)?;
    std::fs::write(
        home.join("paneflow.json"),
        include_str!("agents_window.config.json"),
    )?;
    unsafe {
        std::env::set_var(paneflow_home::HOME_ENV, &home);
        std::env::set_var("PANEFLOW_ALLOW_MULTIPLE", "1");
    }
    Ok(())
}

/// The exports share process-wide state (the config home, the decorations
/// override), so they run one at a time.
static EXCLUSIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
#[ignore = "writes the landing scene export; run with PANEFLOW_SCENE_OUT set"]
fn export_agents_window() -> Result<()> {
    let _exclusive = EXCLUSIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let out = out_dir()?;
    export(
        &out,
        "agents-window",
        agents_window::fixture(),
        &agents_window::GRIDS,
        |_, _| Ok(()),
    )
}

#[test]
#[ignore = "writes the landing scene export; run with PANEFLOW_SCENE_OUT and PANEFLOW_SCENE_SDK_REPO set"]
fn export_changes_window() -> Result<()> {
    let _exclusive = EXCLUSIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let out = out_dir()?;
    let repo = changes_window::repository(&out)?;
    let cwd = repo.display().to_string();
    export(
        &out,
        "changes-window",
        changes_window::fixture(&repo),
        &changes_window::GRIDS,
        |cx, window| {
            cx.update(|cx| {
                window.update(cx, |app, _, cx| {
                    app.open_diff_dock_for_test(cwd.clone(), changes_window::DOCK_WIDTH, cx)
                })
            })?;
            // The dock builds on a background thread with the git CLI.
            let started = std::time::Instant::now();
            let paths = loop {
                cx.run_until_parked();
                let outcome = cx
                    .update(|cx| window.read_with(cx, |app, _| app.diff_dock_outcome_for_test()))?;
                match outcome {
                    Some(Ok(paths)) => break paths,
                    Some(Err(error)) => return Err(anyhow!("the diff dock failed: {error}")),
                    None => {}
                }
                anyhow::ensure!(
                    started.elapsed() < std::time::Duration::from_secs(60),
                    "the diff dock never finished building"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            eprintln!("scene: diff dock shows {} files", paths.len());
            let (top, past) = changes_window::TOP_FILE;
            cx.update(|cx| {
                window.update(cx, |app, _, cx| {
                    app.frame_diff_dock_for_test(changes_window::OPEN_FILE, top, past, cx)
                })
            })?
            .map_err(|error| anyhow!(error))
        },
    )
}

/// Mounts `fixture` in a headless Paneflow window, lets `stage` finish
/// setting it up, then writes the frame and its scene document to
/// `out/name`.
fn export(
    out: &std::path::Path,
    name: &str,
    fixture: fixture::WindowFixture,
    grids: &[(usize, usize)],
    stage: impl FnOnce(&mut HeadlessAppContext, gpui::WindowHandle<crate::PaneFlowApp>) -> Result<()>,
) -> Result<()> {
    isolate_home(out)?;
    let outer = size(
        px(WINDOW_WIDTH + 2. * FRAME_INSET),
        px(WINDOW_HEIGHT + 2. * FRAME_INSET),
    );
    let device = size(
        DevicePixels(outer.width.as_f32() as i32),
        DevicePixels(outer.height.as_f32() as i32),
    );
    crate::window_chrome::csd::override_decorations_for_test(Some(gpui::Decorations::Client {
        tiling: gpui::Tiling::default(),
    }));
    let text = Arc::new(spy::SpyTextSystem::new(gpui_wgpu::CosmicTextSystem::new(
        "IBM Plex Sans",
    )));
    let recorder = spy::Recorder::default();
    let mut cx = HeadlessAppContext::with_platform(
        text.clone() as Arc<dyn PlatformTextSystem>,
        Arc::new(crate::assets::Assets),
        recorder.renderer_factory(device),
    );
    cx.update(|cx| {
        crate::theme::set_active_theme(Some("Tailwind Dark"));
        crate::window_state::record_agents_section(crate::window_state::AgentsSectionState {
            height: None,
            collapsed: true,
        });
        cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
        crate::assets::Assets.load_fonts(cx)
    })?;
    let window = HeadlessAppContext::open_window(&mut cx, outer, |window, cx| {
        window.set_scale_factor(1.0);
        crate::app::window::mount_paneflow_app(window, cx)
    })?;
    cx.run_until_parked();
    let mounted =
        cx.update(|cx| window.update(cx, |app, _, cx| fixture::install(app, fixture, cx)))?;
    cx.update_window(window.into(), |_, window, cx| {
        if let Some(pane) = mounted.panes.get(mounted.focused) {
            let handle = gpui::Focusable::focus_handle(pane.read(cx), cx);
            handle.focus(window, cx);
        }
        window.refresh();
    })?;
    cx.run_until_parked();
    stage(&mut cx, window)?;
    cx.update_window(window.into(), |_, window, _| window.refresh())?;
    cx.run_until_parked();
    // The VT streams place text by column, so they only hold on the grids
    // they were written for.
    cx.update(|cx| -> Result<()> {
        anyhow::ensure!(
            mounted.terminals.len() == grids.len(),
            "one grid per terminal"
        );
        for ((terminal, _), &expected) in mounted.terminals.iter().zip(grids) {
            let size = terminal
                .read(cx)
                .window_size_for_test()
                .ok_or_else(|| anyhow!("a terminal was never laid out"))?;
            anyhow::ensure!(
                (size.cols, size.rows) == expected,
                "terminal grid {}x{} differs from the fixture's {expected:?}",
                size.cols,
                size.rows,
            );
        }
        Ok(())
    })?;
    cx.update(|cx| {
        for (terminal, output) in &mounted.terminals {
            terminal.update(cx, |view, cx| {
                view.terminal.write_output(output);
                view.apply_backend_wakeup(cx);
            });
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(700));
    cx.update(|cx| {
        for (terminal, _) in &mounted.terminals {
            terminal.update(cx, |view, cx| view.apply_backend_wakeup(cx));
        }
    });
    // Unfocused panes fade their dim layer in on the simulated clock.
    for _ in 0..3 {
        cx.update_window(window.into(), |_, window, _| window.refresh())?;
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.advance_clock(std::time::Duration::from_millis(400));
    }
    cx.update_window(window.into(), |_, window, _| window.refresh())?;
    cx.run_until_parked();
    let image = cx
        .capture_screenshot(window.into())
        .context("capturing the headless frame")?;
    let image = image::imageops::crop_imm(
        &image,
        FRAME_INSET as u32,
        FRAME_INSET as u32,
        WINDOW_WIDTH as u32,
        WINDOW_HEIGHT as u32,
    )
    .to_image();
    write_scene(out, name, &image, &recorder, &text)?;
    cx.update(|cx| cx.shutdown());
    Ok(())
}

fn write_scene(
    out: &std::path::Path,
    name: &str,
    image: &image::RgbaImage,
    recorder: &spy::Recorder,
    text: &spy::SpyTextSystem,
) -> Result<()> {
    use gpui::AssetSource as _;

    let scene = recorder
        .scene
        .lock()
        .clone()
        .ok_or_else(|| anyhow!("no frame was rendered"))?;
    let atlas = recorder
        .atlas
        .lock()
        .clone()
        .ok_or_else(|| anyhow!("no atlas was created"))?;
    let tiles = atlas.tiles.lock().clone();
    let started = std::time::Instant::now();
    let export = export::build(export::Input {
        scene: &scene,
        tiles: &tiles,
        text,
        inset: FRAME_INSET,
        width: WINDOW_WIDTH,
        height: WINDOW_HEIGHT,
    })?;
    let dir = out.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(dir.join("icons"))?;
    std::fs::create_dir_all(dir.join("images"))?;
    std::fs::write(
        dir.join("scene.json"),
        serde_json::to_string_pretty(&export.document)?,
    )?;
    for icon in &export.icons {
        let data = crate::assets::Assets
            .load(icon)?
            .ok_or_else(|| anyhow!("missing icon asset {icon}"))?;
        let file = dir.join("icons").join(icon.replace('/', "__"));
        std::fs::write(file, data)?;
    }
    for (id, [x, y, w, h]) in &export.images {
        let crop = image::imageops::crop_imm(
            image,
            x.floor() as u32,
            y.floor() as u32,
            (x + w).ceil() as u32 - x.floor() as u32,
            (y + h).ceil() as u32 - y.floor() as u32,
        )
        .to_image();
        crop.save(dir.join("images").join(format!("{id}.png")))?;
    }
    image.save(dir.join("frame@1x.png"))?;
    let elements = export.document["elements"].as_array().map_or(0, Vec::len);
    eprintln!(
        "scene: {elements} elements, {} icons, {} images in {:?}",
        export.icons.len(),
        export.images.len(),
        started.elapsed()
    );
    for line in &export.report {
        eprintln!("scene: {line}");
    }
    Ok(())
}
