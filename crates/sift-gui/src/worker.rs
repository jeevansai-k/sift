//! Run blocking engine work on a thread and deliver its messages on the GTK
//! main loop.

use gtk::glib;

/// `work` runs on a new thread and may `send` any number of messages;
/// `on_msg` receives them on the main thread, in order.
pub fn run<M: Send + 'static>(work: impl FnOnce(&dyn Fn(M)) + Send + 'static, on_msg: impl Fn(M) + 'static) {
    let (tx, rx) = async_channel::unbounded::<M>();
    std::thread::spawn(move || {
        let send = |m: M| {
            let _ = tx.send_blocking(m);
        };
        work(&send);
    });
    glib::spawn_future_local(async move {
        while let Ok(m) = rx.recv().await {
            on_msg(m);
        }
    });
}
