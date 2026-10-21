//! Work done away from the UI thread whose result comes back to an entity: on the background
//! executor ([`Background::background_then`]), or from another thread's channel
//! ([`Background::forward`]).

use gpui_kit::*;

/// Background work for an entity, through its [`Context`].
pub trait Background<T: 'static> {
    /// Run `work` on the background executor, then `then` with its result on the entity,
    /// unless it was released meanwhile.
    fn background_then<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut T, R, &mut Context<T>) + 'static,
    ) -> Task<()>;

    /// [`Self::background_then`], where `then` also gets the entity's `window` (unless it was
    /// closed meanwhile).
    fn background_then_in<R: Send + 'static>(
        &self,
        window: &Window,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut T, R, &mut Window, &mut Context<T>) + 'static,
    ) -> Task<()>;

    /// Hand each item of `rx` to `f` on the entity as it comes, until `rx` closes or the
    /// entity is released.
    fn forward<E: 'static>(
        &self,
        rx: smol::channel::Receiver<E>,
        f: impl FnMut(&mut T, E, &mut Context<T>) + 'static,
    ) -> Task<()>;
}

impl<T: 'static> Background<T> for Context<'_, T> {
    fn background_then<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut T, R, &mut Context<T>) + 'static,
    ) -> Task<()> {
        self.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            this.update(cx, |this, cx| then(this, result, cx)).ok();
        })
    }

    fn background_then_in<R: Send + 'static>(
        &self,
        window: &Window,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut T, R, &mut Window, &mut Context<T>) + 'static,
    ) -> Task<()> {
        self.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            this.update_in(cx, |this, window, cx| then(this, result, window, cx))
                .ok();
        })
    }

    fn forward<E: 'static>(
        &self,
        rx: smol::channel::Receiver<E>,
        mut f: impl FnMut(&mut T, E, &mut Context<T>) + 'static,
    ) -> Task<()> {
        self.spawn(async move |this, cx| {
            while let Ok(item) = rx.recv().await {
                if this.update(cx, |this, cx| f(this, item, cx)).is_err() {
                    break;
                }
            }
        })
    }
}
