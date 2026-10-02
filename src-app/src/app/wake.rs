#[derive(Clone)]
pub(crate) struct AppWake(smol::channel::Sender<()>);

impl AppWake {
    pub(crate) fn channel() -> (AppWake, smol::channel::Receiver<()>) {
        let (tx, rx) = smol::channel::bounded(1);
        (AppWake(tx), rx)
    }

    pub(crate) fn notify(&self) {
        let _ = self.0.try_send(());
    }
}
