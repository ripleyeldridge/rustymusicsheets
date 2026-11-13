//! NativeActivity host. No global Activity pointer survives Activity recreation.
use crate::{
    model::Paths,
    source::{DocumentAccess, DocumentRef},
};
use android_activity::AndroidApp;
use anyhow::Result;
use std::sync::Arc;

pub mod documents;
mod system_ui;

/// Android's native glue invokes this Rust-ABI symbol on its app thread.
#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    if let Err(error) = run(app) {
        log::error!("RustyMusicSheets startup failed: {error:#}");
    }
}

fn run(app: AndroidApp) -> Result<()> {
    run_with(app, |cc, paths, access, initial| {
        Ok(Box::new(crate::app::MusicApp::new_mobile(
            cc, paths, access, initial,
        )?))
    })
}

/// Host construction is separate from the shared UI, allowing target-only checks.
pub fn run_with(
    app: AndroidApp,
    create: impl FnOnce(
            &eframe::CreationContext<'_>,
            Paths,
            Arc<dyn DocumentAccess>,
            Result<Option<DocumentRef>>,
        ) -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>>
        + 'static,
) -> Result<()> {
    let documents = Arc::new(documents::AndroidDocuments::new(app.clone()));
    let paths = documents.private_paths()?;
    let initial = documents.launch_document();
    let options = eframe::NativeOptions {
        android_app: Some(app.clone()),
        run_and_return: false,
        viewport: eframe::egui::ViewportBuilder::default().with_title("RustyMusicSheets"),
        ..Default::default()
    };
    eframe::run_native(
        "RustyMusicSheets",
        options,
        Box::new(move |cc| {
            let inner = create(cc, paths, documents, initial)?;
            Ok(Box::new(system_ui::InsetApp::new(inner, app)))
        }),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}
