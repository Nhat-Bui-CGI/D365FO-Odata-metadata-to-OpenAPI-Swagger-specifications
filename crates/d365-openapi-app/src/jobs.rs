//! Runs pipeline work off the UI thread. Events are sent back over a channel
//! and egui is asked to repaint so progress shows immediately.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;

pub use d365_openapi_desktop::pipeline::Event;

pub enum Msg {
    Event(Event),
    /// A job finished; the text is shown in the status bar.
    Finished(Result<String, String>),
}

pub struct Ctx {
    pub tx: Sender<Msg>,
    pub egui: eframe::egui::Context,
    pub cancel: Arc<AtomicBool>,
}

impl Ctx {
    pub fn emit(&self, e: Event) {
        let _ = self.tx.send(Msg::Event(e));
        self.egui.request_repaint();
    }
}

pub fn spawn(ctx: Ctx, job: impl FnOnce(&Ctx) -> Result<String, String> + Send + 'static) {
    std::thread::spawn(move || {
        let result = job(&ctx);
        let _ = ctx.tx.send(Msg::Finished(result));
        ctx.egui.request_repaint();
    });
}
