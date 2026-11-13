use super::documents::AndroidDocuments;
use crate::safe_area::{RawBounds, SafeArea};
use android_activity::AndroidApp;
use anyhow::Result;
use eframe::egui::{self, pos2, Rect};
use jni::{
    objects::{JObject, JValue},
    JNIEnv,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(super) struct InsetApp {
    inner: Box<dyn eframe::App>,
    app: AndroidApp,
    snapshot: Arc<Mutex<Option<SafeArea>>>,
    pending: Arc<AtomicBool>,
    warned: Arc<AtomicBool>,
    bounds: RawBounds,
}

impl InsetApp {
    pub fn new(inner: Box<dyn eframe::App>, app: AndroidApp) -> Self {
        Self {
            inner,
            app,
            snapshot: Arc::new(Mutex::new(None)),
            pending: Arc::new(AtomicBool::new(false)),
            warned: Arc::new(AtomicBool::new(false)),
            bounds: RawBounds::default(),
        }
    }

    fn request_measurement(&self, ctx: &egui::Context) {
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let app = self.app.clone();
        let snapshot = self.snapshot.clone();
        let pending = self.pending.clone();
        let warned = self.warned.clone();
        let ctx = ctx.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            let result = AndroidDocuments::new(app).with_env(measure);
            match result {
                Ok(Some(safe)) => {
                    let mut stored = snapshot.lock().unwrap();
                    if *stored != Some(safe) {
                        *stored = Some(safe);
                        log::debug!("Android safe area: {safe:?}");
                        ctx.request_repaint();
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    if !warned.swap(true, Ordering::Relaxed) {
                        log::warn!("Cannot measure Android system insets: {error:#}");
                    }
                }
            }
            pending.store(false, Ordering::Release);
        }));
    }
}

impl eframe::App for InsetApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.inner.raw_input_hook(ctx, input);
        let safe = *self.snapshot.lock().unwrap();
        self.bounds.apply(input, safe);
        self.request_measurement(ctx);
    }
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.inner.update(ctx, frame);
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.inner.save(storage);
    }
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.inner.on_exit(gl);
    }
    fn auto_save_interval(&self) -> std::time::Duration {
        self.inner.auto_save_interval()
    }
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
    }
    fn persist_egui_memory(&self) -> bool {
        self.inner.persist_egui_memory()
    }
}

fn java_rect(env: &mut JNIEnv<'_>, rect: &JObject<'_>) -> Result<Rect> {
    let mut field = |name| -> Result<f32> { Ok(env.get_field(rect, name, "I")?.i()? as f32) };
    Ok(Rect::from_min_max(
        pos2(field("left")?, field("top")?),
        pos2(field("right")?, field("bottom")?),
    ))
}

fn measure(env: &mut JNIEnv<'_>, activity: &JObject<'_>) -> Result<Option<SafeArea>> {
    let window = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
        .l()?;
    let decor = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
        .l()?;
    let content = env
        .call_method(
            activity,
            "findViewById",
            "(I)Landroid/view/View;",
            &[JValue::Int(0x01020002)],
        )?
        .l()?;
    if content.is_null() {
        return Ok(None);
    }
    let width = env.call_method(&content, "getWidth", "()I", &[])?.i()? as f32;
    let height = env.call_method(&content, "getHeight", "()I", &[])?.i()? as f32;
    if width <= 0.0 || height <= 0.0 {
        return Ok(None);
    }
    let location = env.new_int_array(2)?;
    env.call_method(
        &content,
        "getLocationOnScreen",
        "([I)V",
        &[JValue::Object(&location)],
    )?;
    let mut xy = [0i32; 2];
    env.get_int_array_region(&location, 0, &mut xy)?;
    let surface = Rect::from_min_size(pos2(xy[0] as f32, xy[1] as f32), egui::vec2(width, height));
    let visible = if AndroidApp::sdk_version() >= 30 {
        let root_insets = env
            .call_method(
                &decor,
                "getRootWindowInsets",
                "()Landroid/view/WindowInsets;",
                &[],
            )?
            .l()?;
        if root_insets.is_null() {
            return Ok(None);
        }
        let manager = env
            .call_method(
                activity,
                "getWindowManager",
                "()Landroid/view/WindowManager;",
                &[],
            )?
            .l()?;
        let metrics = env
            .call_method(
                &manager,
                "getCurrentWindowMetrics",
                "()Landroid/view/WindowMetrics;",
                &[],
            )?
            .l()?;
        let bounds = env
            .call_method(&metrics, "getBounds", "()Landroid/graphics/Rect;", &[])?
            .l()?;
        let bounds = java_rect(env, &bounds)?;
        let mut mask = 0;
        for kind in ["systemBars", "displayCutout", "ime"] {
            mask |= env
                .call_static_method("android/view/WindowInsets$Type", kind, "()I", &[])?
                .i()?;
        }
        let insets = env
            .call_method(
                &root_insets,
                "getInsets",
                "(I)Landroid/graphics/Insets;",
                &[JValue::Int(mask)],
            )?
            .l()?;
        let inset = java_rect(env, &insets)?;
        Rect::from_min_max(
            bounds.min + inset.min.to_vec2(),
            bounds.max - inset.max.to_vec2(),
        )
    } else {
        let visible = env.new_object("android/graphics/Rect", "()V", &[])?;
        env.call_method(
            &decor,
            "getWindowVisibleDisplayFrame",
            "(Landroid/graphics/Rect;)V",
            &[JValue::Object(&visible)],
        )?;
        java_rect(env, &visible)?
    };
    if !visible.is_positive() {
        return Ok(None);
    }
    Ok(Some(SafeArea { surface, visible }))
}
